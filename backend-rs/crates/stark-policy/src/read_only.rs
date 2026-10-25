use std::collections::BTreeSet;

const MAX_PARSE_CHARS: usize = 20_000;

const READ_ONLY_PROGRAMS: &[&str] = &[
    "ls",
    "dir",
    "cat",
    "head",
    "tail",
    "wc",
    "find",
    "grep",
    "rg",
    "which",
    "where",
    "echo",
    "printf",
    "pwd",
    "env",
    "printenv",
    "date",
    "whoami",
    "id",
    "hostname",
    "uname",
    "stat",
    "file",
    "tree",
    "du",
    "df",
    "sort",
    "uniq",
    "cut",
    "diff",
    "cmp",
    "md5sum",
    "sha256sum",
    "true",
    "false",
];

const GIT_READ_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "remote",
    "ls-files",
    "rev-parse",
    "describe",
    "blame",
    "shortlog",
    "reflog",
    "config",
];

const GIT_VALUE_FLAGS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--exec-path",
    "--config-env",
];

const WRAPPER_PROGRAMS: &[&str] = &[
    "env", "nohup", "time", "command", "exec", "sudo", "builtin", "nice",
];

const SUBSHELL_PROGRAMS: &[&str] = &["bash", "sh", "zsh", "dash", "powershell", "pwsh", "cmd"];

const PYTHON_WRITE_HINTS: &[&str] = &[
    "open(",
    ".write(",
    ".writelines(",
    "os.remove",
    "os.unlink",
    "os.rmdir",
    "os.mkdir",
    "os.makedirs",
    "os.rename",
    "os.replace",
    "shutil.",
    "rmtree",
    "subprocess",
    "os.system",
    "os.popen",
    "savefig",
    "to_csv",
    "to_excel",
    ".unlink(",
    ".rename(",
    "chmod",
    "chown",
];

pub fn check_read_only(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Ok(());
    }
    let text: String = command.chars().take(MAX_PARSE_CHARS).collect();
    if text.contains("$(") || text.contains('`') {
        return Err("command substitution is not verifiably read-only".to_string());
    }
    let (segments, has_redirection, parse_ok) = split_segments(&text);
    if has_redirection {
        return Err("output redirection writes to a file".to_string());
    }
    if !parse_ok {
        return Err("unbalanced quoting cannot be verified read-only".to_string());
    }

    for segment in segments {
        let core = unwrap(&tokenize(&segment));
        if core.is_empty() {
            continue;
        }
        let program = basename(&core[0]);
        let args = &core[1..];
        if SUBSHELL_PROGRAMS.contains(&program.as_str()) {
            return Err(format!("nested shell '{program}' is not read-only"));
        }
        if matches!(program.as_str(), "python" | "python3" | "py") {
            if args.first().is_some_and(|arg| arg == "-c") && args.len() > 1 {
                if python_has_write_hint(&args[1..]) {
                    return Err("python -c contains write operations".to_string());
                }
                continue;
            }
            return Err("python execution is not verifiably read-only".to_string());
        }
        if program == "git" {
            let (subcommand, index) = first_positional(args, GIT_VALUE_FLAGS);
            if !GIT_READ_SUBCOMMANDS.contains(&subcommand.as_str()) {
                return Err(format!("git {subcommand} may write"));
            }
            let tail: Vec<&str> = args[index + 1..]
                .iter()
                .map(String::as_str)
                .filter(|arg| !arg.starts_with('-'))
                .collect();
            match subcommand.as_str() {
                "branch" if !tail.is_empty() => {
                    return Err("git branch with arguments may write".to_string());
                }
                "remote"
                    if !tail.is_empty()
                        && !matches!(tail.first().copied(), Some("show" | "get-url")) =>
                {
                    return Err(format!("git remote {} may write", tail[0]));
                }
                "config"
                    if !args.iter().any(|arg| {
                        matches!(
                            arg.as_str(),
                            "--get" | "--get-all" | "--get-regexp" | "--list" | "-l"
                        )
                    }) =>
                {
                    return Err("git config without a read flag may write".to_string());
                }
                _ => {}
            }
            continue;
        }
        if program == "find"
            && args
                .iter()
                .any(|arg| matches!(arg.as_str(), "-delete" | "-exec" | "-execdir" | "-ok"))
        {
            return Err("find with -delete/-exec is not read-only".to_string());
        }
        if !READ_ONLY_PROGRAMS.contains(&program.as_str()) {
            return Err(format!("'{program}' is not on the read-only allowlist"));
        }
    }
    Ok(())
}

pub fn is_read_only_command(command: &str) -> bool {
    check_read_only(command).is_ok()
}

fn python_has_write_hint(args: &[String]) -> bool {
    let joined = args.join(" ").to_lowercase();
    PYTHON_WRITE_HINTS.iter().any(|hint| joined.contains(hint))
}

fn split_segments(command: &str) -> (Vec<String>, bool, bool) {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_redirection = false;
    let mut parse_ok = true;
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if let Some(active_quote) = quote {
            current.push(character);
            if character == '\\' && active_quote == '"' && index + 1 < chars.len() {
                current.push(chars[index + 1]);
                index += 2;
                continue;
            }
            if character == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            current.push(character);
            index += 1;
            continue;
        }
        if character == '\\' && index + 1 < chars.len() {
            current.push(character);
            current.push(chars[index + 1]);
            index += 2;
            continue;
        }
        if character == '\n' || character == ';' {
            segments.push(std::mem::take(&mut current));
            index += 1;
            continue;
        }
        if character == '&' || character == '|' {
            if index + 1 < chars.len() && chars[index + 1] == character {
                segments.push(std::mem::take(&mut current));
                index += 2;
                continue;
            }
            segments.push(std::mem::take(&mut current));
            index += 1;
            continue;
        }
        if character == '>' {
            has_redirection = true;
        }
        current.push(character);
        index += 1;
    }

    if quote.is_some() {
        parse_ok = false;
    }
    segments.push(current);

    (
        segments
            .into_iter()
            .map(|segment| segment.trim().to_string())
            .filter(|segment| !segment.is_empty())
            .collect(),
        has_redirection,
        parse_ok,
    )
}

fn tokenize(segment: &str) -> Vec<String> {
    shlex::split(segment)
        .unwrap_or_else(|| segment.split_whitespace().map(str::to_string).collect())
}

fn unwrap(tokens: &[String]) -> Vec<String> {
    let mut index = 0;
    let wrapper_names: BTreeSet<&str> = WRAPPER_PROGRAMS.iter().copied().collect();
    while index < tokens.len() {
        let token = &tokens[index];
        if is_assignment(token) {
            index += 1;
            continue;
        }
        if wrapper_names.contains(basename(token).as_str()) {
            index += 1;
            while index < tokens.len()
                && (tokens[index].starts_with('-') || is_assignment(&tokens[index]))
            {
                index += 1;
            }
            continue;
        }
        break;
    }
    tokens[index..].to_vec()
}

fn is_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn first_positional(args: &[String], value_flags: &[&str]) -> (String, usize) {
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if value_flags.contains(&token.as_str()) {
            index += 2;
            continue;
        }
        if token.starts_with('-') {
            index += 1;
            continue;
        }
        return (token.clone(), index);
    }
    (String::new(), args.len())
}

fn basename(program: &str) -> String {
    let program = program.replace('\\', "/");
    let program = program.trim_end_matches('/');
    let mut name = program.rsplit('/').next().unwrap_or(program).to_lowercase();
    if let Some(stripped) = name.strip_suffix(".exe") {
        name = stripped.to_string();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_read_only_commands() {
        assert!(is_read_only_command("ls -la"));
        assert!(is_read_only_command("git status --short"));
        assert!(is_read_only_command("git diff -- src/main.rs"));
        assert!(is_read_only_command("git config --get user.name"));
        assert!(is_read_only_command("rg update_plan backend-rs"));
        assert!(is_read_only_command("python -c \"print(1)\""));
    }

    #[test]
    fn rejects_write_redirects_and_shells() {
        assert!(check_read_only("echo hi > out.txt").is_err());
        assert!(check_read_only("bash -c \"ls\"").is_err());
        assert!(check_read_only("python script.py").is_err());
        assert!(check_read_only("python -c \"open('x','w')\"").is_err());
    }

    #[test]
    fn rejects_write_git_forms() {
        assert!(check_read_only("git push origin main").is_err());
        assert!(check_read_only("git branch feature").is_err());
        assert!(check_read_only("git remote add origin url").is_err());
        assert!(check_read_only("git config user.name x").is_err());
    }

    #[test]
    fn rejects_write_programs_and_find_flags() {
        assert!(check_read_only("rm -rf build").is_err());
        assert!(check_read_only("find . -delete").is_err());
        assert!(check_read_only("find . -exec rm {} +").is_err());
        assert!(check_read_only("cargo test").is_err());
    }

    #[test]
    fn rejects_unbalanced_quotes_and_substitutions() {
        assert!(check_read_only("echo 'unterminated").is_err());
        assert!(check_read_only("echo $(cat secret)").is_err());
        assert!(check_read_only("echo `date`").is_err());
    }
}

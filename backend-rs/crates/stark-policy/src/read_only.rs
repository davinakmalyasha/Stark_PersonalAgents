use std::collections::BTreeSet;

const MAX_PARSE_CHARS: usize = 20_000;

/// Codex's known-safe command set. These are the commands that may run in a
/// read-only sandbox and therefore may be parallelized with other reads.
const SAFE_PROGRAMS: &[&str] = &[
    "cat", "cd", "cut", "echo", "expr", "false", "grep", "head", "id", "ls", "nl", "paste", "pwd",
    "rev", "seq", "stat", "tail", "tr", "true", "uname", "uniq", "wc", "which", "whoami",
];

const FIND_UNSAFE_OPTIONS: &[&str] = &[
    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fls", "-fprint", "-fprint0", "-fprintf",
];

const RG_UNSAFE_OPTIONS_WITH_ARGS: &[&str] = &["--pre", "--hostname-bin"];
const RG_UNSAFE_OPTIONS_WITHOUT_ARGS: &[&str] = &["--search-zip", "-z"];
const BASE64_UNSAFE_OPTIONS: &[&str] = &["-o", "--output"];

const GIT_SAFE_SUBCOMMANDS: &[&str] = &["status", "log", "diff", "show", "branch"];
const GIT_UNSAFE_GLOBAL_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "-p",
    "--config-env",
    "--exec-path",
    "--git-dir",
    "--namespace",
    "--paginate",
    "--super-prefix",
    "--work-tree",
];
const GIT_UNSAFE_SUBCOMMAND_OPTIONS: &[&str] = &["--output", "--ext-diff", "--textconv", "--exec"];

const WRAPPER_PROGRAMS: &[&str] = &["nohup", "time", "command", "builtin", "nice"];

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
        // Variable expansion can smuggle an arbitrary value into an otherwise
        // safe command; Codex's safe-command path is conservative here too.
        if core
            .iter()
            .any(|arg| arg.contains('$') || arg.contains('%') && arg.ends_with('%'))
        {
            return Err("variable expansion is not verifiably read-only".to_string());
        }
        if !is_known_safe_command(&core) {
            let program = basename(&core[0]);
            return Err(format!(
                "'{program}' is not on the Codex safe-command allowlist"
            ));
        }
    }
    Ok(())
}

pub fn is_read_only_command(command: &str) -> bool {
    check_read_only(command).is_ok()
}

/// Codex `is_known_safe_command`: the command will not write, delete, execute
/// another arbitrary program, or redirect output. Path-qualified program names
/// are deliberately rejected so `./sed` cannot impersonate `sed`.
pub fn is_known_safe_command(command: &[String]) -> bool {
    let Some(cmd0) = command.first() else {
        return false;
    };
    if cmd0.contains('/') || cmd0.contains('\\') {
        return false;
    }

    let program = basename(cmd0);
    let args = &command[1..];

    // `bash -lc "ls && pwd"` is safe only when every nested plain command is
    // safe. The outer shell itself is the harness's execution shell.
    if matches!(program.as_str(), "bash" | "sh" | "zsh") {
        let script = args
            .iter()
            .position(|arg| matches!(arg.as_str(), "-c" | "-lc"))
            .and_then(|index| args.get(index + 1));
        return script.is_some_and(|script| check_read_only(script).is_ok());
    }
    if matches!(program.as_str(), "powershell" | "pwsh" | "cmd") {
        return false;
    }

    if SAFE_PROGRAMS.contains(&program.as_str()) {
        return true;
    }

    match program.as_str() {
        "base64" => !args.iter().any(|arg| {
            BASE64_UNSAFE_OPTIONS.contains(&arg.as_str())
                || arg.starts_with("--output=")
                || (arg.starts_with("-o") && arg != "-o")
        }),
        "find" => !args
            .iter()
            .any(|arg| FIND_UNSAFE_OPTIONS.contains(&arg.as_str())),
        "rg" => !args.iter().any(|arg| {
            RG_UNSAFE_OPTIONS_WITHOUT_ARGS.contains(&arg.as_str())
                || RG_UNSAFE_OPTIONS_WITH_ARGS
                    .iter()
                    .any(|option| arg == option || arg.starts_with(&format!("{option}=")))
        }),
        "git" => is_safe_git_command(args),
        "sed" => {
            command.len() <= 4
                && command.get(1).map(String::as_str) == Some("-n")
                && is_valid_sed_n_arg(command.get(2).map(String::as_str))
        }
        _ => false,
    }
}

fn is_safe_git_command(args: &[String]) -> bool {
    let subcommand_index = args.iter().position(|arg| !arg.starts_with('-'));
    let Some(subcommand_index) = subcommand_index else {
        return false;
    };
    let global_args = &args[..subcommand_index];
    if global_args.iter().any(|arg| {
        GIT_UNSAFE_GLOBAL_OPTIONS
            .iter()
            .any(|option| arg == option || arg.starts_with(&format!("{option}=")))
    }) {
        return false;
    }
    let subcommand = args[subcommand_index].as_str();
    if !GIT_SAFE_SUBCOMMANDS.contains(&subcommand) {
        return false;
    }
    let subcommand_args = &args[subcommand_index + 1..];
    if subcommand_args.iter().any(|arg| {
        GIT_UNSAFE_SUBCOMMAND_OPTIONS
            .iter()
            .any(|option| arg == option || arg.starts_with(&format!("{option}=")))
    }) {
        return false;
    }
    match subcommand {
        "branch" => git_branch_is_read_only(subcommand_args),
        _ => true,
    }
}

fn git_branch_is_read_only(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }
    let mut saw_read_only_flag = false;
    for arg in args {
        match arg.as_str() {
            "--list" | "-l" | "--show-current" | "-a" | "--all" | "-r" | "--remotes" | "-v"
            | "-vv" | "--verbose" => saw_read_only_flag = true,
            _ if arg.starts_with("--format=") => saw_read_only_flag = true,
            _ => return false,
        }
    }
    saw_read_only_flag
}

fn is_valid_sed_n_arg(arg: Option<&str>) -> bool {
    let Some(arg) = arg else {
        return false;
    };
    let Some(body) = arg.strip_suffix('p') else {
        return false;
    };
    let mut seen_comma = false;
    let mut digits = 0usize;
    for character in body.chars() {
        if character.is_ascii_digit() {
            digits += 1;
        } else if character == ',' && !seen_comma {
            seen_comma = true;
        } else {
            return false;
        }
    }
    digits > 0
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
    fn allows_codex_safe_commands() {
        assert!(is_read_only_command("ls -la"));
        assert!(is_read_only_command("git status --short"));
        assert!(is_read_only_command("git diff -- src/main.rs"));
        assert!(is_read_only_command("git branch --show-current"));
        assert!(is_read_only_command("rg update_plan backend-rs"));
        assert!(is_read_only_command("find . -name file.txt"));
        assert!(is_read_only_command("sed -n '1,5p' file.txt"));
        assert!(is_read_only_command("base64 file.txt"));
        assert!(is_read_only_command("bash -lc \"ls && pwd\""));
    }

    #[test]
    fn rejects_unknown_and_mutating_programs() {
        assert!(check_read_only("python -c \"print(1)\"").is_err());
        assert!(check_read_only("python script.py").is_err());
        assert!(check_read_only("rm -rf build").is_err());
        assert!(check_read_only("cargo test").is_err());
        assert!(check_read_only("echo hi > out.txt").is_err());
        assert!(check_read_only("bash -c \"ls && rm -rf /\"").is_err());
    }

    #[test]
    fn rejects_codex_unsafe_git_forms() {
        assert!(check_read_only("git push origin main").is_err());
        assert!(check_read_only("git -C . status").is_err());
        assert!(check_read_only("git --git-dir=.evil diff").is_err());
        assert!(check_read_only("git branch feature").is_err());
        assert!(check_read_only("git remote add origin url").is_err());
        assert!(check_read_only("git config user.name x").is_err());
    }

    #[test]
    fn rejects_codex_unsafe_command_options() {
        assert!(check_read_only("find . -delete").is_err());
        assert!(check_read_only("find . -exec rm {} +").is_err());
        assert!(check_read_only("rg --pre cmd pattern").is_err());
        assert!(check_read_only("rg -z pattern").is_err());
        assert!(check_read_only("base64 -o out.txt file").is_err());
        assert!(check_read_only("./sed -n '1p' file").is_err());
    }

    #[test]
    fn rejects_unbalanced_quotes_and_substitutions() {
        assert!(check_read_only("echo 'unterminated").is_err());
        assert!(check_read_only("echo $(cat secret)").is_err());
        assert!(check_read_only("echo `date`").is_err());
    }

    #[test]
    fn rejects_environment_dumps_and_variable_expansion() {
        assert!(check_read_only("env").is_err());
        assert!(check_read_only("printenv").is_err());
        assert!(check_read_only("echo $OPENAI_API_KEY").is_err());
        assert!(check_read_only("echo %PATH%").is_err());
    }
}

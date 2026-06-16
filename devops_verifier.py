import json
import yaml
import re

def validate_yaml(content: str) -> list[str]:
    """Validate YAML syntax using PyYAML."""
    try:
        yaml.safe_load(content)
        return []
    except Exception as e:
        return [f"YAML Syntax Error: {e}"]

def validate_json(content: str) -> list[str]:
    """Validate JSON syntax using json module."""
    try:
        json.loads(content)
        return []
    except Exception as e:
        return [f"JSON Syntax Error: {e}"]

def validate_dockerfile(content: str) -> list[str]:
    """Check Dockerfile against pinned version and non-root USER standards."""
    errors = []
    lines = content.splitlines()
    has_user = False
    
    for idx, line in enumerate(lines, 1):
        line_strip = line.strip()
        if line_strip.startswith("#") or not line_strip:
            continue
            
        # Check base image tag pinning
        if line_strip.upper().startswith("FROM "):
            image_part = line_strip[5:].strip()
            # If AS multi-stage builds are used
            if " AS " in image_part.upper():
                parts = re.split(r'\s+AS\s+', image_part, flags=re.IGNORECASE)
                image_part = parts[0].strip()
            
            if ":" not in image_part:
                errors.append(f"Line {idx}: Base image '{image_part}' must specify an explicit version tag (do not omit tags).")
            elif image_part.endswith(":latest"):
                errors.append(f"Line {idx}: Base image '{image_part}' uses forbidden ':latest' tag. Pin an exact version instead.")
                
        # Check USER directive
        if line_strip.upper().startswith("USER "):
            has_user = True
            
    if not has_user:
        errors.append("Dockerfile is missing a USER directive. For production-grade security, run as a non-root user (e.g., 'USER node' or 'USER app').")
        
    return errors

def detect_secret_leak(content: str) -> list[str]:
    """Scan content for potential secret/token leakage."""
    errors = []
    patterns = {
        "GitHub Token": r"(?:ghp|gho|ghu|ghs|ghr)_[a-zA-Z0-9]{36,255}",
        "Private Key": r"-----BEGIN [A-Z ]+ PRIVATE KEY-----",
        "AWS Access Key ID": r"AKIA[0-9A-Z]{16}",
        "Slack Webhook": r"https://hooks\.slack\.com/services/T[a-zA-Z0-9_]+/B[a-zA-Z0-9_]+/[a-zA-Z0-9_]+"
    }
    for name, pattern in patterns.items():
        if re.search(pattern, content):
            errors.append(f"Potential secret leakage detected ({name}). Do not hardcode raw credentials in configuration files.")
            
    # Generic credential detection pattern
    generic_secret_pattern = r"(?i)(?:key|secret|password|token|pwd)\s*[:=]\s*['\"]([^'\"]{16,})['\"]"
    for match in re.finditer(generic_secret_pattern, content):
        val = match.group(1)
        val_lower = val.lower()
        # Filter out obvious dummies
        is_dummy = val_lower in ["password", "secret", "token", "your_key", "your_secret", "placeholder"] or \
                   any(d in val_lower for d in ["your_", "mock_", "dummy_", "test_", "_here", "_placeholder"])
        if not is_dummy and len(set(val)) > 6:
            errors.append(f"Potential hardcoded credential '{val[:4]}...{val[-4:]}' detected. Use environment variables instead.")
            break
            
    return errors

def validate_kubernetes_manifest(content: str) -> list[str]:
    """Scan Kubernetes manifests for security and resource constraints."""
    errors = []
    try:
        docs = list(yaml.safe_load_all(content))
    except Exception:
        return []
        
    for doc in docs:
        if not isinstance(doc, dict):
            continue
        if "apiVersion" in doc and "kind" in doc:
            kind = doc.get("kind", "")
            if kind in ("Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob", "Pod"):
                spec = doc.get("spec", {})
                if kind != "Pod":
                    template = spec.get("template", {}) if isinstance(spec, dict) else {}
                    pod_spec = template.get("spec", {}) if isinstance(template, dict) else {}
                else:
                    pod_spec = spec
                    
                containers = pod_spec.get("containers", []) if isinstance(pod_spec, dict) else []
                for container in containers:
                    if not isinstance(container, dict):
                        continue
                    cname = container.get("name", "unknown")
                    
                    # 1. Resource CPU/memory check
                    resources = container.get("resources", {})
                    if not resources or "limits" not in resources or "requests" not in resources:
                        errors.append(f"Kubernetes {kind} container '{cname}' is missing resource limits or requests configuration.")
                        
                    # 2. SecurityContext check
                    sec_ctx = container.get("securityContext", {})
                    if not sec_ctx:
                        errors.append(f"Kubernetes {kind} container '{cname}' is missing securityContext settings.")
                    else:
                        if not sec_ctx.get("runAsNonRoot"):
                            errors.append(f"Kubernetes {kind} container '{cname}' should specify 'runAsNonRoot: true' in securityContext.")
                        if not sec_ctx.get("allowPrivilegeEscalation") is False:
                            errors.append(f"Kubernetes {kind} container '{cname}' should specify 'allowPrivilegeEscalation: false' in securityContext.")
    return errors

def validate_bash_script(content: str) -> list[str]:
    """Validate basic bash script syntax and shebang requirements."""
    errors = []
    lines = [l.strip() for l in content.splitlines() if l.strip()]
    if not lines:
        return []
        
    # Check shebang
    if not lines[0].startswith("#!"):
        errors.append("Bash script is missing a shebang (e.g., '#!/usr/bin/env bash' on the first line).")
    elif "bash" not in lines[0] and "sh" not in lines[0]:
        errors.append(f"Bash script shebang '{lines[0]}' does not reference bash or sh.")
        
    # Check balanced characters
    for char, name in [("(", "parentheses"), ("[", "brackets"), ("{", "braces")]:
        open_count = content.count(char)
        close_char = ")" if char == "(" else "]" if char == "[" else "}"
        close_count = content.count(close_char)
        if open_count != close_count:
            errors.append(f"Sanity Check: Unbalanced {name} (opened {open_count} times, closed {close_count} times).")
            
    return errors

def verify_devops_files(files_written: dict[str, str]) -> list[str]:
    """Run all validation checks on the modified/written DevOps files."""
    errors = []
    for file_path, content in files_written.items():
        fname = file_path.lower()
        
        # 1. Run secret leak scanning on all written files
        secret_errors = detect_secret_leak(content)
        errors.extend([f"[{file_path}] {e}" for e in secret_errors])
        
        # 2. Check file type specific validation
        if fname.endswith((".yml", ".yaml")):
            errs = validate_yaml(content)
            errors.extend([f"[{file_path}] {e}" for e in errs])
            if not errs:
                k8s_errs = validate_kubernetes_manifest(content)
                errors.extend([f"[{file_path}] {e}" for e in k8s_errs])
        elif fname.endswith(".json"):
            errs = validate_json(content)
            errors.extend([f"[{file_path}] {e}" for e in errs])
        elif "dockerfile" in fname:
            errs = validate_dockerfile(content)
            errors.extend([f"[{file_path}] {e}" for e in errs])
        elif fname.endswith((".sh", ".bash")):
            errs = validate_bash_script(content)
            errors.extend([f"[{file_path}] {e}" for e in errs])
            
    return errors

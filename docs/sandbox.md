# Sandbox

Stark exposes the same two axes as Codex:

| Setting | Values | Default |
|---|---|---|
| `STARK_SANDBOX` | `read-only`, `workspace-write`, `danger-full-access` | `workspace-write` |
| `STARK_SANDBOX_NETWORK` | `off`, `on` | `off` |
| `STARK_APPROVAL` | `untrusted`, `on-failure`, `on-request`, `never` | `on-request` |

Every spawned process goes through `stark_sandbox::SandboxProfile`:

- `processes.rs` pipes path → `wrap_tokio_command`
- `processes.rs` PTY path → `env()` applied to the PTY `CommandBuilder`

The resolved mode, network flag, approval policy, and backend are exported to
child processes as `STARK_SANDBOX_MODE`, `STARK_SANDBOX_NETWORK`,
`STARK_APPROVAL`, and stated in the model prompt.

## Current backend status

| Platform | Backend | Enforcement |
|---|---|---|
| Windows | `none (AppContainer pending)` | no |
| Linux | `none (Landlock pending)` | no |
| macOS | `none (Seatbelt pending)` | no |

`stark_sandbox::enforced()` returns false, and the prompt says so explicitly:
the agent is told writes are **not** OS-sandboxed. This is deliberate — a
sandbox that lies is worse than none.

## Implementing a backend

Add it under `stark-sandbox/src/lib.rs::backend`:

- **Windows AppContainer**: create the profile and derive its SID, grant the
  SID modify rights on the workspace, spawn with `STARTUPINFOEX` +
  `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` without `internetClient`.
  This requires replacing std/tokio process spawning with `CreateProcessW`
  and manual pipe wiring, including the PTY path.
- **Linux Landlock**: allow reads, restrict writes to the workspace and temp,
  optionally deny network via seccomp.
- **macOS Seatbelt**: generate a profile and run under `sandbox-exec`.

When a backend lands, flip `enforced()` for that platform and update the
table above; the prompt and startup log update automatically.

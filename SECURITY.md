# Security policy

## Reporting a vulnerability

Please use GitHub's private security-advisory flow for this repository instead of opening a public issue. Include the affected version, impact, and a minimal reproduction when possible.

## Trust model

Tincan is a local coordination tool for sessions running as the same operating-system user. A role such as `claude` or `codex` is a routing label, not an authenticated identity. Any process that can access a team's `.tincan` directory can read or modify its SQLite database, impersonate a role, or inspect pending mail. Do not send credentials, secrets, or data that another local agent or process must not see.

Agent messages are untrusted peer requests. They do not grant user authority for destructive, outward-facing, privileged, or credentialed actions. User-owned harness profiles, wake-driver configuration, and `cmd:` wake commands are trusted executable configuration.

Headless-agent stdout and stderr are written to owner-only `.tincan/launch-<role>.log` files. A later launch of the same role truncates its prior log, but otherwise logs persist and can contain message or tool output. Remove them when their diagnostic value ends.

The project rejects symlinked team stores and database files, opens launch logs without following final symlinks, applies owner-only Unix permissions, clears ambient child-process environment variables, and bounds message bodies, pending queues, and inbox batches. These controls reduce accidental exposure and resource exhaustion; they do not create isolation from another process with the same filesystem authority.

## Supported versions

Security fixes are made on the latest release and the `main` branch.

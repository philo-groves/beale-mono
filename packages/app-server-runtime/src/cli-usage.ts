export function usage(): string {
  return [
    "Usage: appServer protocol describe --json",
    "       appServer session <command> ... --json",
    "       appServer knowledge <command> --input <json> --json",
    "       appServer harness <command> --input <json> --json",
    "       appServer complete --json < request.json",
    "       appServer profile resolve --workspace-root <path> [--profile <path> | --profile-id <id>] --json",
    "       appServer models list [provider] --json",
    "       appServer auth <list|status|verify|logout> [provider]",
    "       appServer tools <list|config> [options] --json",
    "       appServer config <show|set> [options] --json",
    "",
    "This executable is an optional client of the Beale app-server. It cannot host or execute research sessions.",
    "Desktop and iOS clients should use the app-server APIs directly.",
  ].join("\n");
}

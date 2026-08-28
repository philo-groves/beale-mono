export function usage(): string {
  return [
    "Usage: honeycrisp protocol describe --json",
    "       honeycrisp session <command> ... --json",
    "       honeycrisp knowledge <command> --input <json> --json",
    "       honeycrisp harness <command> --input <json> --json",
    "       honeycrisp complete --json < request.json",
    "       honeycrisp profile resolve --workspace-root <path> [--profile <path> | --profile-id <id>] --json",
    "       honeycrisp models list [provider] --json",
    "       honeycrisp auth <list|status|verify|logout> [provider]",
    "       honeycrisp tools <list|config> [options] --json",
    "       honeycrisp config <show|set> [options] --json",
    "",
    "This executable is an optional client of the Beale app-server. It cannot host or execute research sessions.",
    "Desktop and iOS clients should use the app-server APIs directly.",
  ].join("\n");
}

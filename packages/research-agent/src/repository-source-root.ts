import { realpathSync } from "node:fs";
import { basename, isAbsolute, relative, resolve, sep, toNamespacedPath } from "node:path";
import { PRE_BEALE_DATA_DIRECTORY_NAME } from "./legacy-compatibility.js";
import { defaultSourceRepositoryStoreDirectory } from "./source-materializer.js";

function within(root: string, path: string): boolean {
  const pathFromRoot = relative(toNamespacedPath(root), toNamespacedPath(path));
  return pathFromRoot === "" || (pathFromRoot !== ".." && !pathFromRoot.startsWith(`..${sep}`)
    && !isAbsolute(pathFromRoot));
}

/** Managed checkouts live below the Beale data directory; other runtime data is not source. */
export function assertRepositorySourceRoot(root: string): void {
  const canonicalRoot = realpathSync(root);
  const parts = canonicalRoot.split(sep);
  const configuredStore = resolve(defaultSourceRepositoryStoreDirectory());
  let canonicalStore: string;
  try { canonicalStore = realpathSync(configuredStore); }
  catch { canonicalStore = configuredStore; }
  for (let index = 0; index < parts.length; index += 1) {
    const part = parts[index]?.toLowerCase();
    if (part !== ".beale" && part !== PRE_BEALE_DATA_DIRECTORY_NAME) continue;
    const dataRoot = parts.slice(0, index + 1).join(sep) || sep;
    const defaultStore = resolve(dataRoot, "repositories");
    const managedConfiguredStore = [configuredStore, canonicalStore].some((store) =>
      relative(toNamespacedPath(dataRoot), toNamespacedPath(store)) !== "" && within(store, canonicalRoot));
    if (!managedConfiguredStore && !within(defaultStore, canonicalRoot)) {
      const rootParts = toNamespacedPath(canonicalRoot).toLowerCase().split(sep);
      const storeParts = toNamespacedPath(canonicalStore).toLowerCase().split(sep);
      const firstDifferentSegment = rootParts.findIndex((part, position) => part !== storeParts[position]);
      throw new Error(`Repository source tools cannot inspect Beale runtime data outside managed checkouts. Diagnostics: rootName=${basename(canonicalRoot)}, configuredStoreName=${basename(configuredStore)}, canonicalStoreName=${basename(canonicalStore)}, rootSegments=${rootParts.length}, storeSegments=${storeParts.length}, firstDifferentSegment=${firstDifferentSegment}.`);
    }
  }
}

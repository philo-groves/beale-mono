import { WORKSPACE_PRIMARY_DIRECTORY_MISSING_MESSAGE } from '../../shared/ipc';

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function isWorkspacePrimaryDirectoryMissingError(error: unknown): boolean {
  return userFacingErrorMessage(error) === WORKSPACE_PRIMARY_DIRECTORY_MISSING_MESSAGE;
}

export function userFacingErrorMessage(error: unknown): string {
  const message = errorMessage(error)
    .replace(/^Error invoking remote method '[^']+':\s*/i, '')
    .replace(/^Error:\s*/i, '')
    .trim();
  return message || 'An unknown error occurred.';
}

/**
 * usePickerDir.ts — sensible starting directories for the native file dialogs.
 *
 * Without an explicit `defaultPath`, Windows opens the dialog wherever the
 * process happens to be, which for an installed build is the app's own
 * install directory under Program Files. Nobody keeps the files they want to
 * send in there, so every pick started with the user navigating out of a folder
 * full of DLLs.
 *
 * Resolved through `@tauri-apps/api/path` rather than hardcoded, because the
 * real location is not guessable: Downloads can be redirected, the profile can
 * live on another drive, and the folder names are localised.
 *
 * Project: LocalDrop — zero-configuration LAN file and text transfer
 * Author:  Emmanuel Paul <pauldukz@gmail.com>
 */
import { desktopDir, downloadDir, homeDir } from "@tauri-apps/api/path";

/**
 * Cached because the answer cannot change while the app runs, and each lookup
 * is an IPC round trip we would otherwise pay on every click of Browse.
 * `null` is a resolved cache entry meaning "nothing usable found", distinct
 * from `undefined` meaning "not looked up yet".
 */
let cached: string | null | undefined;

/**
 * Best available starting directory for a "pick something to send" dialog.
 *
 * Tried in order of how likely the user's files are to be there. Each call is
 * guarded separately rather than wrapped in one try: on a stripped-down or
 * unusual profile any single one of these can fail while the others succeed,
 * and a shared catch would discard the working ones too.
 *
 * Returns `undefined` if none resolve, which callers pass straight to the
 * dialog — omitting `defaultPath` restores the previous behaviour rather than
 * failing the pick.
 */
export async function pickerStartDir(): Promise<string | undefined> {
  if (cached !== undefined) return cached ?? undefined;

  for (const resolve of [downloadDir, desktopDir, homeDir]) {
    try {
      const dir = await resolve();
      if (dir) {
        cached = dir;
        return dir;
      }
    } catch {
      // This one is unavailable; try the next.
    }
  }

  // Remembered so a permission failure is not retried on every click.
  cached = null;
  return undefined;
}

import { confirm, message } from "@tauri-apps/plugin-dialog";
import { t } from "./i18n";

/* window.confirm / window.alert / window.prompt are silent no-ops in the
 * macOS WebView (wry registers no WKUIDelegate JavaScript-dialog handlers,
 * so WKWebView cancels them without showing anything). Every confirmation
 * must go through the dialog plugin instead; text input goes through the
 * in-app PromptDialogHost (see promptDialog.ts). */

export function confirmDialog(
  text: string,
  kind: "info" | "warning" | "error" = "warning",
): Promise<boolean> {
  return confirm(text, {
    kind,
    okLabel: t("common.ok"),
    cancelLabel: t("common.cancel"),
  });
}

export async function alertDialog(
  text: string,
  kind: "info" | "warning" | "error" = "info",
): Promise<void> {
  await message(text, { kind });
}

import { confirm, message } from "@tauri-apps/plugin-dialog";
import { t } from "./i18n";

/* window.confirm / window.alert / window.prompt are silent no-ops in the
 * macOS WebView (wry registers no WKUIDelegate JavaScript-dialog handlers,
 * so WKWebView cancels them without showing anything). Every confirmation
 * must go through the dialog plugin instead; text input goes through the
 * in-app PromptDialogHost (see promptDialog.ts). */

export async function confirmDialog(
  text: string,
  kind: "info" | "warning" | "error" = "warning",
): Promise<boolean> {
  try {
    return await confirm(text, {
      kind,
      okLabel: t("common.ok"),
      cancelLabel: t("common.cancel"),
    });
  } catch (error) {
    // Every caller reads `false` as "the user said no" and returns without a
    // word, so a broken bridge would otherwise present as a button that does
    // nothing at all. Say so, then decline: never destroy data unconfirmed.
    console.error("Confirmation dialog failed", error);
    await alertDialog(t("common.dialogFailed", { error: String(error) }), "error");
    return false;
  }
}

export async function alertDialog(
  text: string,
  kind: "info" | "warning" | "error" = "info",
): Promise<void> {
  try {
    await message(text, { kind });
  } catch (error) {
    // Nothing left to report the failure with — the console is the last resort.
    console.error("Message dialog failed", error, text);
  }
}

/// <reference types="vite/client" />

declare module "*.vue" {
	import type { DefineComponent } from "vue";

	const component: DefineComponent<Record<string, never>, Record<string, never>, unknown>;
	export default component;
}
interface ImportMetaEnv {
	/** "1" in the Mac App Store build; see src/distribution.ts. */
	readonly VITE_AURATERM_APP_STORE?: string;
}

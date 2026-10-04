// Pure helpers for the UI's config editor. Keep Node fs/Git code out of
// this module so UI components can use it without pulling in persistence code.
export const MAX_CONFIG_BYTES = 64 * 1024;
export const MAX_CONFIG_LABEL = `${MAX_CONFIG_BYTES / 1024} KiB`;

// Formatting only needs valid JSON; Ctrl+S performs the full settings validation.
export function formatConfigJson(text: string): string {
	return `${JSON.stringify(JSON.parse(text), null, 2)}\n`;
}

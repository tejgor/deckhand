export type ExportKind = 'function' | 'const' | 'class' | 'enum' | 'type' | 'default' | 'default-function' | 'star';
export function listExports(file: string): {name: string; kind: ExportKind}[];
export function listTestTitles(file: string): {title: string; line: number; dynamic: boolean}[];

import xtermHeadless from '@xterm/headless';

const {Terminal} = xtermHeadless as typeof import('@xterm/headless');

const DEFAULT_SCROLLBACK = 10_000;

function clampSize(value: number, fallback: number): number {
	if (!Number.isFinite(value)) {
		return fallback;
	}
	return Math.max(1, Math.floor(value));
}

function stripTrailingEmptyLines(lines: string[]): string[] {
	const output = [...lines];
	while (output.length > 0 && output[output.length - 1] === '') {
		output.pop();
	}
	return output;
}

export class TerminalPreview {
	private readonly terminal: InstanceType<typeof Terminal>;
	private snapshot = '';
	private dirty = true;
	private pending: Promise<void> = Promise.resolve();
	private cols: number;
	private rows: number;

	constructor(cols: number, rows: number) {
		this.cols = clampSize(cols, 80);
		this.rows = clampSize(rows, 24);
		this.terminal = new Terminal({
			cols: this.cols,
			rows: this.rows,
			scrollback: DEFAULT_SCROLLBACK,
			allowProposedApi: true,
		});
	}

	write(data: string): Promise<void> {
		this.pending = this.pending
			.then(
				() =>
					new Promise<void>(resolve => {
						this.terminal.write(data, () => {
							this.dirty = true;
							resolve();
						});
					}),
			)
			.catch(() => {
				this.dirty = true;
			});
		return this.pending;
	}

	resize(cols: number, rows: number): Promise<void> {
		const nextCols = clampSize(cols, this.cols);
		const nextRows = clampSize(rows, this.rows);
		if (nextCols === this.cols && nextRows === this.rows) {
			return this.pending;
		}

		this.cols = nextCols;
		this.rows = nextRows;
		this.pending = this.pending
			.then(() => {
				this.terminal.resize(this.cols, this.rows);
				this.dirty = true;
			})
			.catch(() => {
				this.dirty = true;
			});
		return this.pending;
	}

	async getSnapshot(): Promise<string> {
		await this.pending;
		if (this.dirty) {
			this.refreshSnapshot();
			this.dirty = false;
		}
		return this.snapshot;
	}

	async getAnsiFrame(): Promise<string> {
		await this.pending;
		if (this.dirty) {
			this.refreshSnapshot();
			this.dirty = false;
		}

		const buffer = this.terminal.buffer.active;
		const startLine = Math.max(0, buffer.baseY);
		const chunks = ['\x1b[2J\x1b[H'];

		for (let index = 0; index < this.rows; index += 1) {
			const line = buffer.getLine(startLine + index);
			const text = line ? line.translateToString(true) : '';
			if (text) {
				chunks.push(`\x1b[${index + 1};1H${text}`);
			}
		}

		chunks.push(`\x1b[${Math.max(1, buffer.cursorY + 1)};${Math.max(1, buffer.cursorX + 1)}H`);
		return chunks.join('');
	}

	getCachedSnapshot(): string {
		return this.snapshot;
	}

	dispose(): void {
		this.terminal.dispose();
	}

	private refreshSnapshot(): void {
		const buffer = this.terminal.buffer.active;
		const startLine = Math.max(0, buffer.baseY);
		const renderedLines: string[] = [];

		for (let index = 0; index < this.rows; index += 1) {
			const line = buffer.getLine(startLine + index);
			renderedLines.push(line ? line.translateToString(true) : '');
		}

		this.snapshot = stripTrailingEmptyLines(renderedLines).join('\n');
	}
}

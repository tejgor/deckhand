import {execFile} from 'node:child_process';
import {createInterface} from 'node:readline/promises';
import {stdin as input, stdout as output} from 'node:process';
import {promisify} from 'node:util';

const execFileAsync = promisify(execFile);

interface ToolInstall {
	key: string;
	label: string;
	command: string;
	installCommand?: string;
	description: string;
	optional?: boolean;
}

interface ToolStatus extends ToolInstall {
	installed: boolean;
	path?: string;
}

const AGENT_TOOLS: ToolInstall[] = [
	{
		key: 'claude',
		label: 'Claude Code',
		command: 'claude',
		installCommand: 'curl -fsSL https://claude.ai/install.sh | bash',
		description: 'Anthropic Claude Code agent',
	},
	{
		key: 'pi',
		label: 'Pi',
		command: 'pi',
		installCommand: 'npm install -g --ignore-scripts @earendil-works/pi-coding-agent',
		description: 'Pi coding agent',
	},
	{
		key: 'codex',
		label: 'Codex',
		command: 'codex',
		installCommand: 'npm install -g @openai/codex',
		description: 'OpenAI Codex CLI agent',
	},
];

const OPTIONAL_TOOLS: ToolInstall[] = [
	{
		key: 'lazygit',
		label: 'lazygit',
		command: 'lazygit',
		description: 'optional Git tab UI',
		optional: true,
	},
];

function shell(): string {
	return process.env.SHELL || '/bin/bash';
}

async function commandPath(command: string): Promise<string | undefined> {
	try {
		const {stdout} = await execFileAsync(shell(), ['-ic', `command -v ${command}`]);
		return stdout.trim() || undefined;
	} catch {
		return undefined;
	}
}

async function detectTools(): Promise<ToolStatus[]> {
	const tools = [...AGENT_TOOLS, ...OPTIONAL_TOOLS];
	const statuses = await Promise.all(tools.map(async tool => {
		const path = await commandPath(tool.command);
		return {...tool, installed: Boolean(path), path};
	}));
	return statuses;
}

function splitCommand(command: string): {file: string; args: string[]} {
	return {file: shell(), args: ['-ic', command]};
}

async function runInstall(command: string): Promise<number> {
	const {spawn} = await import('node:child_process');
	const {file, args} = splitCommand(command);
	return await new Promise(resolve => {
		const child = spawn(file, args, {stdio: 'inherit'});
		child.on('error', () => resolve(1));
		child.on('close', code => resolve(code ?? 1));
	});
}

function printStatus(statuses: ToolStatus[]): void {
	console.log('Deckhand setup check\n');
	for (const status of statuses) {
		const marker = status.installed ? '✓' : '•';
		const location = status.installed ? ` (${status.path})` : ' (not installed)';
		const optional = status.optional ? ' optional' : '';
		console.log(`${marker} ${status.label}${optional}: ${status.description}${location}`);
	}
	console.log('');
}

async function promptYes(question: string, assumeYes: boolean): Promise<boolean> {
	if (assumeYes) {
		return true;
	}
	const rl = createInterface({input, output});
	try {
		const answer = (await rl.question(`${question} [y/N] `)).trim().toLowerCase();
		return answer === 'y' || answer === 'yes';
	} finally {
		rl.close();
	}
}

export async function runSetup(args: string[]): Promise<void> {
	const assumeYes = args.includes('--yes') || args.includes('-y');
	const checkOnly = args.includes('--check');
	const statuses = await detectTools();
	printStatus(statuses);

	const missing = statuses.filter(status => !status.installed);
	if (missing.length === 0) {
		console.log('Everything Deckhand knows how to check is installed.');
		return;
	}

	const missingAgents = missing.filter(status => status.installCommand);
	if (missingAgents.length === 0) {
		console.log('All supported agents are installed. lazygit is optional; install it separately if you want the Git tab.');
		return;
	}
	if (checkOnly) {
		console.log('Run `deckhand setup` to install missing agents. lazygit is optional and is not installed by setup.');
		return;
	}

	for (const agent of missingAgents) {
		const command = agent.installCommand!;
		console.log(`${agent.label} can be installed with: ${command}`);
		if (await promptYes(`Install ${agent.label} now?`, assumeYes)) {
			const code = await runInstall(command);
			if (code !== 0) {
				console.log(`${agent.label} install failed. You can run this manually: ${command}`);
			}
		}
	}

	const nextStatuses = await detectTools();
	console.log('\nAfter setup:');
	printStatus(nextStatuses);
	const remainingAgents = nextStatuses.filter(status => !status.installed && status.installCommand);
	if (remainingAgents.length > 0) {
		console.log('Some agents are still missing. Restart your terminal if installs succeeded but are not detected yet.');
	}
	if (nextStatuses.some(status => status.key === 'lazygit' && !status.installed)) {
		console.log('lazygit is optional and is not installed by setup; install it separately if you want the Git tab.');
	}
}

// Records src/help.ts HELP_TOPICS as JSON, which deckhand_core::help::HELP_TOPICS must serialise to.
//
//   node crates/core/tests/data/gen_help_topics.mjs > crates/core/tests/data/help_topics.json
//
// Node strips help.ts's types itself (it has no imports).
import process from 'node:process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../..');
const {HELP_TOPICS} = await import(path.join(root, 'src/help.ts'));
process.stdout.write(`${JSON.stringify(HELP_TOPICS)}\n`);

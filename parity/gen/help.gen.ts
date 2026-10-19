/**
 * Parity fixtures for src/help.ts (fixtures/parity/help/). Regenerate with
 *
 *   node --import tsx parity/gen/help.gen.ts
 *
 * help.ts is data: HELP_TOPICS (the ? guide). It is recorded whole, as a zero-argument call, in TS order, so the Rust
 * HELP_TOPICS must serialize to exactly this (variant tags: {keys,text} | {note} | {heading}). HelpLine and HelpTopic
 * are types. tests/help.test.ts exercises helpPane.tsx (another unit), not functions of this module.
 */
import {Recorder, installDeterminism} from '../lib.ts';
import {HELP_TOPICS} from '../../src/help.ts';

installDeterminism();
const recorder = new Recorder('help');
recorder.call('HELP_TOPICS', () => HELP_TOPICS, []);
await recorder.write({prune: true});

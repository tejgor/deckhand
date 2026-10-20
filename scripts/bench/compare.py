#!/usr/bin/env python3
"""Side-by-side performance of the TypeScript build (dist/cli.js) and the native build (target/release/deckhand).

Each build drives its own daemon (protocol v45 vs v46) in a throwaway HOME/DECKHAND_HOME under $TMPDIR, with a fake
`claude` first on PATH, so no real agent, credential or ~/.deckhand file is touched. Scenarios:

  cli       `deckhand hooks codex` cold start (no daemon)
  startup   daemon spawn until it answers a ping
  memory    RSS and phys_footprint of the daemon and its workers with 0, 1 and N idle sessions (agents excluded)
  spawn     `create` request until the agent process is running
  idle      CPU used by the daemon and workers with N idle sessions over a window
  hooks     `deckhand hook` round trip, timed by the agent exactly as Claude/Codex invoke it
  flood     an agent printing M MB of coloured output while a client watches its preview: write time and CPU

Build both first: `npm run build` and `cargo build --release -p deckhand`. Usage:
  python3 scripts/bench/compare.py [--rounds 3] [--sessions 10] [--flood-mb 50] [--json out.json]
"""

import argparse
import json
import os
import re
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NODE = shutil.which('node')
BUILDS = {
    'ts': [NODE, str(ROOT / 'dist/cli.js')],
    'rust': [str(ROOT / 'target/release/deckhand')],
}

AGENT = r'''#!{python}
import json, os, random, subprocess, sys, time
args = sys.argv[1:]
if '--help' in args:  # the daemon's capability probe, as tests/helpers.ts fakeAgent answers it
    print('PROMPT --settings --no-daemon resume')
    sys.exit(0)
if '--version' in args or args[:1] == ['version']:
    print('1.0.0 (Claude Code)')
    sys.exit(0)
bench = os.path.join(os.environ['DECKHAND_HOME'], 'bench')
cfg = json.load(open(os.path.join(bench, 'agent.json')))
pid = os.getpid()

def hook(event):
    body = {{'hook_event_name': event, 'session_id': 'bench', 'tool_name': 'Bash', 'tool_input': {{'command': 'ls'}}}}
    subprocess.run(cfg['hook_cmd'], input=json.dumps(body).encode(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def out(name, data):
    tmp = os.path.join(bench, '.' + name)
    with open(tmp, 'w') as f:
        json.dump(data, f)
    os.replace(tmp, os.path.join(bench, name))

def payload(mb):
    words = 'the quick brown fox jumps over a lazy dog compile test passed warning error src/daemon.rs:142 ✓ ✗ → 日本語'.split()
    rng, buf, i = random.Random(42), bytearray(), 0
    while len(buf) < mb * 1_000_000:
        text = ' '.join(rng.choice(words) for _ in range(rng.randint(4, 16)))
        buf += f'\x1b[{{30 + i % 8}}m{{i:7d}}\x1b[0m {{text}}\r\n'.encode()
        if i % 40 == 39:
            buf += b'\x1b[3A\x1b[2K\x1b[1m redraw \x1b[0m\x1b[3B\r'
        i += 1
    return bytes(buf)

out(f'started-{{pid}}.json', {{'t': time.time()}})
os.write(1, b'\x1b[1mbench agent\x1b[0m ready\r\n')
hook('SessionStart')
if cfg['mode'] == 'hooks':
    ms = []
    for _ in range(cfg['count']):
        t = time.perf_counter()
        hook('PreToolUse')
        ms.append((time.perf_counter() - t) * 1000)
    out(f'hooks-{{pid}}.json', {{'ms': ms}})
elif cfg['mode'] == 'flood':
    data = memoryview(payload(cfg['mb']))
    out(f'ready-{{pid}}.json', {{}})
    while not os.path.exists(os.path.join(bench, 'go')):
        time.sleep(0.01)
    t, done = time.perf_counter(), 0
    while done < len(data):
        done += os.write(1, data[done:done + 65536])
    os.write(1, b'\r\nBENCH-END\r\n')
    out(f'flood-{{pid}}.json', {{'seconds': time.perf_counter() - t, 'bytes': len(data)}})
while True:
    time.sleep(3600)
'''


# ---------------------------------------------------------------- processes

def ps_table():
    """pid -> (ppid, rss_kb, cpu_seconds, command)."""
    rows = {}
    text = subprocess.run(['ps', '-axo', 'pid=,ppid=,rss=,time=,command='], capture_output=True, text=True).stdout
    for line in text.splitlines():
        parts = line.split(None, 4)
        if len(parts) < 5:
            continue
        pid, ppid, rss, cpu, command = parts
        rows[int(pid)] = (int(ppid), int(rss), parse_cpu(cpu), command)
    return rows


def parse_cpu(value):
    seconds = 0.0
    for part in value.split(':'):
        seconds = seconds * 60 + float(part)
    return seconds


def footprints(pids):
    if not pids:
        return {}
    args = ['footprint', '--noCategories', '-f', 'bytes']
    for pid in pids:
        args += ['-p', str(pid)]
    text = subprocess.run(args, capture_output=True, text=True).stdout
    return {int(m[1]): int(m[2]) for m in re.finditer(r'\[(\d+)\]: \S+\s+Footprint: (\d+) B', text)}


class Daemon:
    def __init__(self, build, work):
        self.build = build
        self.home = Path(tempfile.mkdtemp(prefix=f'{build}-', dir=work))
        assert_isolated(self.home)
        self.bench = self.home / 'bench'
        self.bench.mkdir()
        self.fakebin = self.home / 'fakebin'
        self.fakebin.mkdir()
        agent = self.fakebin / 'claude'
        agent.write_text(AGENT.format(python=sys.executable))
        agent.chmod(0o755)
        self.repo = self.home / 'repo'
        self.repo.mkdir()
        git = ['git', '-C', str(self.repo), '-c', 'user.name=bench', '-c', 'user.email=bench@example.invalid', '-c', 'commit.gpgsign=false']
        subprocess.run(git[:3] + ['init', '-q', '-b', 'main'], check=True)
        (self.repo / 'README.md').write_text('bench\n')
        subprocess.run(git + ['add', '.'], check=True)
        subprocess.run(git + ['commit', '-qm', 'init'], check=True)
        self.env = {
            'HOME': str(self.home), 'DECKHAND_HOME': str(self.home), 'DECKHAND_DEV': '0', 'DECKHAND_AGENT_LATEST': '{}',
            'PATH': os.pathsep.join([str(self.fakebin), os.path.dirname(NODE), '/usr/bin', '/bin']),
            'SHELL': '/bin/sh', 'TERM': 'xterm-256color', 'LANG': 'en_US.UTF-8', 'TMPDIR': tempfile.gettempdir(),
        }
        self.socket = str(self.home / 'daemon.sock')
        self.proc = None
        self.seen = set()

    def cli(self, *args):
        return BUILDS[self.build] + list(args)

    def start(self):
        """Spawns the daemon and returns the seconds until it answers a ping."""
        t = time.perf_counter()
        self.proc = subprocess.Popen(self.cli('--daemon'), cwd=self.repo, env=self.env,
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                     start_new_session=True)
        while True:
            try:
                Conn(self.socket).request({'type': 'ping'}, timeout=2)
                return time.perf_counter() - t
            except OSError:
                if self.proc.poll() is not None:
                    raise RuntimeError(f'{self.build} daemon exited: {self.proc.returncode}')
                if time.perf_counter() - t > 30:
                    raise RuntimeError(f'{self.build} daemon did not answer within 30 s')
                time.sleep(0.002)

    def configure_agent(self, **cfg):
        (self.bench / 'agent.json').write_text(json.dumps({'hook_cmd': self.cli('hook'), **cfg}))

    def session(self, session_id):
        return next(s for s in Conn(self.socket).request({'type': 'list'}) if s['id'] == session_id)

    def create(self, title, cols=120, rows=40):
        before = set(self.bench.glob('started-*.json'))
        t0 = time.time()
        record = Conn(self.socket).request({'type': 'create', 'input': {
            'title': title, 'program': 'claude', 'cwd': str(self.repo), 'repoRoot': str(self.repo),
            'cols': cols, 'rows': rows, 'worktreeMode': 'none'}}, timeout=60)
        started = wait_for(lambda: set(self.bench.glob('started-*.json')) - before, 30, f'{title} agent start')
        path = started.pop()
        agent_pid = int(path.stem.split('-')[1])
        return record['id'], agent_pid, json.loads(path.read_text())['t'] - t0

    def processes(self):
        """The daemon and every worker it owns (tree descendants plus workers/*.pid), excluding the fake agents."""
        table = ps_table()
        children = {}
        for pid, (ppid, *_rest) in table.items():
            children.setdefault(ppid, []).append(pid)
        roots = [self.proc.pid] + [int(p.read_text().strip()) for p in (self.home / 'workers').glob('*.pid') if p.read_text().strip().isdigit()]
        found, stack = {}, [pid for pid in roots if pid in table]
        while stack:
            pid = stack.pop()
            if pid in found:
                continue
            command = table[pid][3]
            if str(self.fakebin) in command:
                self.seen.add(pid)
                continue
            found[pid] = table[pid]
            stack += children.get(pid, [])
        self.seen |= set(found)
        return found

    def measure(self):
        procs = self.processes()
        fp = footprints(list(procs))
        def kind(pid, command):
            # Node rewrites its command line to process.title (deckhand-session-worker, ...).
            if pid == self.proc.pid:
                return 'daemon'
            for name in ('session-worker', 'workspace-worker'):
                if f'--{name}' in command or f'deckhand-{name}' in command:
                    return name
            return 'other'
        out = {'rss_mb': 0.0, 'footprint_mb': 0.0, 'cpu_s': 0.0, 'by_kind': {}}
        for pid, (_ppid, rss, cpu, command) in procs.items():
            k = out['by_kind'].setdefault(kind(pid, command), {'count': 0, 'rss_mb': 0.0, 'footprint_mb': 0.0})
            k['count'] += 1
            k['rss_mb'] += rss / 1024
            k['footprint_mb'] += fp.get(pid, 0) / 2**20
            out['rss_mb'] += rss / 1024
            out['footprint_mb'] += fp.get(pid, 0) / 2**20
            out['cpu_s'] += cpu
        return out

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.processes()
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                pass
        for pid in self.seen | {self.proc.pid if self.proc else -1}:
            try:
                os.kill(pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        if self.proc:
            self.proc.wait()
        shutil.rmtree(self.home, ignore_errors=True)


def assert_isolated(path):
    real = Path(os.path.realpath(Path.home()))
    resolved = Path(os.path.realpath(path))
    for name in ['.deckhand', '.deckhand-dev', '.deckhand-rust-dev', '.claude', '.codex']:
        protected = real / name
        if resolved == protected or protected in resolved.parents or resolved in protected.parents:
            raise SystemExit(f'refusing to run against real state: {resolved}')


def wait_for(read, timeout, what):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = read()
        if value:
            return value
        time.sleep(0.01)
    raise RuntimeError(f'timed out waiting for {what}')


class Conn:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.buf = b''

    def send(self, message):
        self.sock.sendall((json.dumps(message) + '\n').encode())

    def lines(self):
        while True:
            while b'\n' in self.buf:
                line, self.buf = self.buf.split(b'\n', 1)
                if line.strip():
                    yield json.loads(line)
            chunk = self.sock.recv(1 << 16)
            if not chunk:
                return
            self.buf += chunk

    def request(self, message, timeout=15):
        request_id = str(uuid.uuid4())
        self.sock.settimeout(timeout)
        self.send({**message, 'requestId': request_id})
        for payload in self.lines():
            if payload.get('type') == 'response' and payload.get('requestId') == request_id:
                if not payload.get('ok'):
                    raise RuntimeError(f"{message['type']}: {payload.get('error')}")
                self.sock.close()
                return payload.get('data')
        raise OSError('connection closed before response')


def watch_preview(daemon, session_id, marker):
    """Holds a preview subscription open like a UI would; returns (conn, state) where state['seen'] is the
    perf_counter time the preview first showed `marker`."""
    conn = Conn(daemon.socket)
    conn.send({'type': 'watch-preview', 'requestId': str(uuid.uuid4()), 'sessionId': session_id, 'cols': 120, 'rows': 40})
    state = {'updates': 0, 'seen': None}
    def drain():
        try:
            for message in conn.lines():
                preview = message.get('preview') or message.get('data') or {}
                state['updates'] += 1
                if state['seen'] is None and marker in (preview.get('content') or ''):
                    state['seen'] = time.perf_counter()
        except (OSError, ValueError):
            return
    threading.Thread(target=drain, daemon=True).start()
    return conn, state


class Ui:
    """The interactive UI in a 160x48 pseudo-terminal, connected to the daemon of the same home."""

    def __init__(self, daemon):
        import fcntl, pty, struct, termios
        self.output = bytearray()
        self.started = time.perf_counter()
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(daemon.repo)
            os.execve(daemon.cli()[0], daemon.cli(), daemon.env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack('HHHH', 48, 160, 0, 0))
        os.kill(pid, signal.SIGWINCH)
        self.pid, self.fd = pid, fd
        daemon.seen.add(pid)
        threading.Thread(target=self._drain, daemon=True).start()

    def _drain(self):
        while True:
            try:
                chunk = os.read(self.fd, 1 << 16)
            except OSError:
                return
            if not chunk:
                return
            self.output += chunk

    def first_frame(self, timeout=30):
        """Seconds from spawn until the sidebar's 'Sessions' title was drawn."""
        def drawn():
            text = re.sub(rb'\x1b\[[0-9;?]*[A-Za-z]', b'', bytes(self.output))
            return b'Sessions' in text and time.perf_counter()
        return wait_for(drawn, timeout, 'first UI frame') - self.started

    def close(self):
        try:
            os.write(self.fd, b'q')
            wait_for(lambda: os.waitpid(self.pid, os.WNOHANG)[0], 5, 'UI exit')
        except (OSError, RuntimeError):
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)
        os.close(self.fd)


def cpu_of(daemon, pids):
    table = ps_table()
    return sum(table[pid][2] for pid in pids if pid in table)


# ---------------------------------------------------------------- scenarios

def bench_cli(build, runs):
    ms = []
    with tempfile.TemporaryDirectory(prefix='cli-') as home:
        env = {'HOME': home, 'DECKHAND_HOME': home, 'PATH': os.pathsep.join([os.path.dirname(NODE), '/usr/bin', '/bin'])}
        command = BUILDS[build] + ['hooks', 'codex']
        subprocess.run(command, env=env, stdout=subprocess.DEVNULL, check=True)  # warm the file cache
        for _ in range(runs):
            t = time.perf_counter()
            subprocess.run(command, env=env, stdout=subprocess.DEVNULL, check=True)
            ms.append((time.perf_counter() - t) * 1000)
    return {'median_ms': statistics.median(ms), 'min_ms': min(ms)}


def bench_round(build, work, args):
    result = {}
    daemon = Daemon(build, work)
    try:
        result['startup_ms'] = daemon.start() * 1000
        time.sleep(2)
        result['memory'] = {'0': daemon.measure()}

        daemon.configure_agent(mode='idle')
        spawn = []
        for i in range(args.sessions):
            _sid, _agent, seconds = daemon.create(f'idle {i}')
            spawn.append(seconds * 1000)
            if i == 0:
                time.sleep(3)
                result['memory']['1'] = daemon.measure()
        time.sleep(3)
        result['memory'][str(args.sessions)] = daemon.measure()
        result['spawn_ms'] = {'median': statistics.median(spawn), 'first': spawn[0]}

        pids = list(daemon.processes())
        before = cpu_of(daemon, pids)
        time.sleep(args.idle_seconds)
        result['idle'] = {'cpu_s': cpu_of(daemon, pids) - before, 'window_s': args.idle_seconds}

        ui = Ui(daemon)
        try:
            result['ui'] = {'first_frame_ms': ui.first_frame() * 1000}
            time.sleep(3)
            table, fp = ps_table(), footprints([ui.pid])
            result['ui'].update(rss_mb=table[ui.pid][1] / 1024, footprint_mb=fp.get(ui.pid, 0) / 2**20)
            before = cpu_of(daemon, [ui.pid])
            time.sleep(args.idle_seconds)
            result['ui']['idle_cpu_s'] = cpu_of(daemon, [ui.pid]) - before
        finally:
            ui.close()

        daemon.configure_agent(mode='hooks', count=args.hooks)
        sid, agent, _ = daemon.create('hooks')
        hooks = json.loads(wait_for(lambda: (daemon.bench / f'hooks-{agent}.json').exists() and (daemon.bench / f'hooks-{agent}.json').read_text(), 300, 'hooks'))['ms']
        attention = daemon.session(sid).get('attention') or {}
        if attention.get('event') != 'PreToolUse':
            raise RuntimeError(f'{build}: the daemon did not record the hooks (attention={attention})')
        hooks = hooks[2:]  # first calls warm the file cache
        result['hooks_ms'] = {'median': statistics.median(hooks), 'p95': sorted(hooks)[int(len(hooks) * 0.95) - 1]}

        daemon.configure_agent(mode='flood', mb=args.flood_mb)
        before_procs = set(daemon.processes())
        sid, agent, _ = daemon.create('flood')
        conn, preview = watch_preview(daemon, sid, 'BENCH-END')
        flood_file = daemon.bench / f'flood-{agent}.json'
        wait_for(lambda: (daemon.bench / f'ready-{agent}.json').exists(), 120, 'flood payload')
        time.sleep(1)
        tracked = [pid for pid in daemon.processes() if pid not in before_procs] + [daemon.proc.pid]
        start_cpu = cpu_of(daemon, tracked)
        go = time.perf_counter()
        (daemon.bench / 'go').write_text('')
        flood = json.loads(wait_for(lambda: flood_file.exists() and flood_file.read_text(), 600, 'flood'))
        wait_for(lambda: preview['seen'], 600, 'preview to show the end of the flood')
        time.sleep(1)  # let the last snapshot's CPU land in ps
        end_cpu = cpu_of(daemon, tracked)
        conn.sock.close()
        result['flood'] = {
            'mb': flood['bytes'] / 1e6, 'write_s': flood['seconds'], 'mb_per_s': flood['bytes'] / 1e6 / flood['seconds'],
            'cpu_s': end_cpu - start_cpu, 'processed_s': preview['seen'] - go, 'preview_updates': preview['updates'],
        }
        result['peak'] = daemon.measure()
    finally:
        daemon.stop()
    return result


# ---------------------------------------------------------------- report

def med(values):
    return statistics.median(values) if values else float('nan')


def summarize(rounds, cli, args):
    def pick(build, getter):
        return med([getter(r) for r in rounds[build]])
    n = str(args.sessions)
    rows = [
        ('CLI cold start, `hooks codex` (ms)', lambda b: cli[b]['median_ms']),
        ('Daemon startup to first ping (ms)', lambda b: pick(b, lambda r: r['startup_ms'])),
        ('Hook round trip, median (ms)', lambda b: pick(b, lambda r: r['hooks_ms']['median'])),
        ('Hook round trip, p95 (ms)', lambda b: pick(b, lambda r: r['hooks_ms']['p95'])),
        ('Session spawn, median (ms)', lambda b: pick(b, lambda r: r['spawn_ms']['median'])),
        ('Memory, daemon alone (MB footprint)', lambda b: pick(b, lambda r: r['memory']['0']['footprint_mb'])),
        ('Memory, 1 session (MB footprint)', lambda b: pick(b, lambda r: r['memory']['1']['footprint_mb'])),
        (f'Memory, {n} sessions (MB footprint)', lambda b: pick(b, lambda r: r['memory'][n]['footprint_mb'])),
        (f'Memory, {n} sessions (MB RSS)', lambda b: pick(b, lambda r: r['memory'][n]['rss_mb'])),
        (f'Idle CPU, {n} sessions (% of a core)', lambda b: pick(b, lambda r: 100 * r['idle']['cpu_s'] / r['idle']['window_s'])),
        (f'Flood {args.flood_mb} MB: agent write time (s)', lambda b: pick(b, lambda r: r['flood']['write_s'])),
        (f'Flood {args.flood_mb} MB: throughput (MB/s)', lambda b: pick(b, lambda r: r['flood']['mb_per_s'])),
        (f'Flood {args.flood_mb} MB: until the preview shows the end (s)',lambda b: pick(b, lambda r: r['flood']['processed_s'])),
        (f'Flood {args.flood_mb} MB: CPU used (s)', lambda b: pick(b, lambda r: r['flood']['cpu_s'])),
        ('UI startup to first frame (ms)', lambda b: pick(b, lambda r: r['ui']['first_frame_ms'])),
        ('UI memory (MB footprint)', lambda b: pick(b, lambda r: r['ui']['footprint_mb'])),
        ('UI memory (MB RSS)', lambda b: pick(b, lambda r: r['ui']['rss_mb'])),
        (f'UI idle CPU, {n} sessions listed (% of a core)', lambda b: pick(b, lambda r: 100 * r['ui']['idle_cpu_s'] / r['idle']['window_s'])),
    ]
    width = max(len(label) for label, _ in rows)
    print(f"\n{'':{width}}  {'TypeScript':>12}  {'Rust':>12}  {'ratio':>8}")
    for label, getter in rows:
        ts, rs = getter('ts'), getter('rust')
        ratio = ts / rs if rs else float('inf')
        print(f'{label:{width}}  {ts:12.1f}  {rs:12.1f}  {ratio:7.1f}x')
    print('\nratio = TypeScript / Rust (for throughput, < 1 means Rust is faster).')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--sessions', type=int, default=10)
    parser.add_argument('--hooks', type=int, default=40)
    parser.add_argument('--idle-seconds', type=int, default=20)
    parser.add_argument('--flood-mb', type=int, default=50)
    parser.add_argument('--cli-runs', type=int, default=30)
    parser.add_argument('--json', help='also write every raw measurement here')
    args = parser.parse_args()
    for build, command in BUILDS.items():
        if not Path(command[-1]).exists():
            raise SystemExit(f'{build} build missing: {command[-1]}')
    # Short base directory: daemon.sock must fit macOS's 104-byte socket path limit.
    work = tempfile.mkdtemp(prefix='dhb-')
    try:
        cli = {build: bench_cli(build, args.cli_runs) for build in BUILDS}
        rounds = {build: [] for build in BUILDS}
        for i in range(args.rounds):
            order = list(BUILDS) if i % 2 == 0 else list(reversed(BUILDS))
            for build in order:
                print(f'round {i + 1}/{args.rounds}: {build}', file=sys.stderr, flush=True)
                rounds[build].append(bench_round(build, work, args))
    finally:
        shutil.rmtree(work, ignore_errors=True)
    if args.json:
        Path(args.json).write_text(json.dumps({'cli': cli, 'rounds': rounds, 'args': vars(args)}, indent=2))
    summarize(rounds, cli, args)


if __name__ == '__main__':
    main()

"""Check original tasks and reference edits in disposable offline containers."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def grade(task, workspace, grader, image):
    name = 'check.py' if task['language'] == 'python' else 'check.cjs' if task['language'] == 'javascript' else 'held_out.rs'
    (grader / name).write_text(task['held_out'])
    if task['language'] == 'rust':
        command = ['sh', '-c', 'mkdir -p tests && cp /grader/held_out.rs tests/held_out.rs && cargo test --offline --quiet']
    else:
        command = ['python3', '-B', '/grader/check.py'] if task['language'] == 'python' else ['node', '/grader/check.cjs']
    args = ['docker', 'run', '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
            '--pids-limit', '128', '--memory', '768m', '--cpus', '2', '--user', f'{os.getuid()}:{os.getgid()}',
            '--tmpfs', '/tmp:rw,nosuid,nodev,size=128m', '--env', 'PYTHONPATH=/workspace', '--env', 'PYTHONDONTWRITEBYTECODE=1',
            '--mount', f'type=bind,src={workspace},dst=/workspace', '--mount', f'type=bind,src={grader},dst=/grader,readonly',
            '--workdir', '/workspace', '--entrypoint', command[0], image, *command[1:]]
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=90)
    if len(result.stdout) > 1048576:
        raise ValueError('grader output exceeded bound')
    return {'passed': result.returncode == 0, 'exit': result.returncode, 'output': result.stdout.decode('utf-8', 'replace')}


def validate(corpus, image, output):
    rows = []
    for task in corpus['tasks']:
        with tempfile.TemporaryDirectory(prefix='craftsman-corpus-') as temp:
            root = Path(temp); workspace = root / 'workspace'; grader = root / 'grader'
            workspace.mkdir(); grader.mkdir(); root.chmod(0o755)
            for relative, content in task['files'].items():
                target = workspace / relative
                if relative.startswith('/') or '..' in Path(relative).parts:
                    raise ValueError('task path escaped workspace')
                target.parent.mkdir(parents=True, exist_ok=True);target.write_text(content)
            baseline = grade(task, workspace, grader, image)
            reference_workspace = root / 'reference'
            reference_workspace.mkdir()
            for relative, content in {**task['files'], **task['reference']}.items():
                target = reference_workspace / relative;target.parent.mkdir(parents=True, exist_ok=True);target.write_text(content)
            reference = grade(task, reference_workspace, grader, image)
            row = {'id': task['id'], 'language': task['language'], 'category': task['category'],
                   'baseline_passed': baseline['passed'], 'reference_passed': reference['passed']}
            rows.append(row)
            if not reference['passed'] or baseline['passed']:
                print(json.dumps({**row, 'baseline': baseline['output'][-3000:], 'reference': reference['output'][-3000:]}), flush=True)
            else:
                print(json.dumps(row), flush=True)
    report = {'model_requests': 0, 'tasks': rows, 'all_references_pass': all(r['reference_passed'] for r in rows),
              'all_originals_fail': all(not r['baseline_passed'] for r in rows)}
    output.write_text(json.dumps(report, indent=2) + '\n')
    if not report['all_references_pass'] or not report['all_originals_fail']:
        raise SystemExit(1)


if __name__ == '__main__':
    parser = argparse.ArgumentParser();parser.add_argument('--image', required=True);parser.add_argument('--corpus', type=Path, required=True);parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    validate(json.loads(args.corpus.read_text()), args.image, args.output)

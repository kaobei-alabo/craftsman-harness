"""Same-model product comparison, externally isolated with the shared gateway."""
import json
import os
from pathlib import Path
import subprocess
import sys


def run(bootstrap):
    selected=next(iter(bootstrap['config']['providers'].values()));model='gpt-6.1-sol'
    settings=selected['models'][model].get('settings',{})
    effort=settings.get('reasoning',{}).get('value','medium')
    conf=Path('/home/eval/.codex');conf.mkdir(parents=True,exist_ok=True)
    # No host auth, plugins, hooks or connections are copied into this task.
    text='\n'.join([
        'model = '+json.dumps(model), 'model_provider = "eval"', 'model_reasoning_effort = '+json.dumps(effort),
        'web_search = "disabled"', '[model_providers.eval]', 'name = "GPT"', 'wire_api = "responses"',
        'base_url = '+json.dumps(selected['base_url']), 'env_key = "CRAFTSMAN_EVAL_KEY"',
        'request_max_retries = 0', 'stream_max_retries = 0', 'supports_websockets = false',
    ])+'\n'
    path=conf/'config.toml';path.write_text(text);path.chmod(0o600)
    env=dict(os.environ);env['CRAFTSMAN_EVAL_KEY']=selected['api_key']
    command=['codex','exec','--json','--skip-git-repo-check','--dangerously-bypass-approvals-and-sandbox',bootstrap['prompt']]
    # The container, network and gateway are the execution/billing boundaries.
    process=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=sys.stderr,env=env,stdin=subprocess.DEVNULL)
    final=None;frames=0
    for line in process.stdout:
        frames+=1
        if len(line)>1024*1024 or frames>10000:process.terminate();break
        event=json.loads(line);print(json.dumps({'kind':'codex','event':event},ensure_ascii=False),flush=True)
        if event.get('type')=='turn.completed':final=event
    code=process.wait(timeout=35)
    print(json.dumps({'kind':'result','final':{'result':{'stopReason':'end_turn'}} if final else None,'agent_exit':code,'approvals_once':0,'human_questions':0,'frames':frames}),flush=True)
    return code


if __name__=='__main__':
    raw=sys.stdin.buffer.read(1024*1024+1)
    if len(raw)>1024*1024:raise SystemExit('Evaluation bootstrap is too large.')
    raise SystemExit(run(json.loads(raw)))

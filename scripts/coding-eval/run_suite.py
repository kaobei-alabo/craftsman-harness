"""A local Docker evaluation runner using ACP; no craftsman exec command."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import selectors
import shutil
import stat
import subprocess
import sys
import time
sys.path.insert(0,str(Path(__file__).resolve().parent))
from check_corpus import grade
from test_scope import retained_checks
from budget import money


def snapshot_files(root):
    """Read a stopped task through no-follow directory handles, with finite bounds."""
    result={};rejected=[];entries=0;size=0
    flags=os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW
    def walk(fd,prefix):
        nonlocal entries,size
        for entry in os.scandir(fd):
            entries+=1
            if entries>1024:raise ValueError('Evaluation file count exceeded its bound')
            if entry.name in {'target','__pycache__','.git'}:continue
            relative=prefix+entry.name
            info=entry.stat(follow_symlinks=False)
            if stat.S_ISDIR(info.st_mode):
                child=os.open(entry.name,flags,dir_fd=fd)
                try:walk(child,relative+'/')
                finally:os.close(child)
            elif stat.S_ISREG(info.st_mode) and info.st_nlink==1 and info.st_size<=1024*1024:
                handle=os.open(entry.name,os.O_RDONLY|os.O_NOFOLLOW,dir_fd=fd)
                try:
                    actual=os.fstat(handle)
                    if not stat.S_ISREG(actual.st_mode) or actual.st_nlink!=1 or (actual.st_dev,actual.st_ino)!=(info.st_dev,info.st_ino):raise ValueError('Evaluation file identity changed')
                    data=bytearray()
                    while chunk:=os.read(handle,65536):
                        data.extend(chunk);size+=len(chunk)
                        if len(data)>1024*1024 or size>8*1024*1024:raise ValueError('Evaluation source size exceeded its bound')
                    result[relative]=bytes(data)
                finally:os.close(handle)
            else:rejected.append(relative)
    handle=os.open(root,flags)
    try:walk(handle,'')
    finally:os.close(handle)
    return result,rejected


def docker(*args, timeout=60):
    return subprocess.check_output(['docker',*args],stderr=subprocess.STDOUT,timeout=timeout).decode().strip()


def capture(command, payload, name, seconds=300):
    process=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    process.stdin.write(json.dumps(payload).encode());process.stdin.close()
    poll=selectors.DefaultSelector();poll.register(process.stdout,selectors.EVENT_READ,'stdout');poll.register(process.stderr,selectors.EVENT_READ,'stderr')
    streams={'stdout':bytearray(),'stderr':bytearray()};start=time.monotonic();failure=None
    try:
        while poll.get_map():
            if time.monotonic()-start>seconds:raise TimeoutError('Evaluation task reached its wall limit.')
            for key,_ in poll.select(0.1):
                chunk=os.read(key.fd,16384)
                if not chunk:poll.unregister(key.fileobj);continue
                streams[key.data].extend(chunk)
                if sum(map(len,streams.values()))>8*1024*1024:raise ValueError('Evaluation output reached its bound.')
        code=process.wait(timeout=5)
    except (TimeoutError,ValueError) as error:
        failure=str(error);docker('rm','--force',name,timeout=15)
        try:code=process.wait(timeout=5)
        except subprocess.TimeoutExpired:process.kill();code=process.wait(timeout=5)
    finally:poll.close()
    return {key:value.decode('utf-8','replace') for key,value in streams.items()},code,round(time.monotonic()-start,3),failure


def base_args(image,name,network,workspace,driver,arm='craftsman'):
    client='codex_client.py' if arm=='codex' else 'acp_client.py'
    return ['docker','run','--name',name,'--interactive','--network',network,'--read-only','--cap-drop','ALL','--security-opt','no-new-privileges',
            '--pids-limit','128','--memory','768m','--cpus','2','--user',f'{os.getuid()}:{os.getgid()}',
            '--tmpfs',f'/home/eval:rw,nosuid,nodev,uid={os.getuid()},gid={os.getgid()},size=128m',
            '--tmpfs','/tmp:rw,nosuid,nodev,size=128m','--env','HOME=/home/eval','--env','CRAFTSMAN_CONFIG_DIR=/home/eval/.craftsman',
            '--mount',f'type=bind,src={workspace},dst=/workspace','--mount',f'type=bind,src={driver/client},dst=/driver/{client},readonly',
            '--workdir','/workspace','--entrypoint','python3',image,'-I','/driver/'+client]


def source_config(path,model):
    raw=path.read_bytes()
    if len(raw)>1024*1024:raise ValueError('Config exceeded evaluation bound')
    config=json.loads(raw);provider,api_model=model.split('/',1)
    selected=config['providers'][provider]
    if selected['protocol']!='openai_responses' or api_model!='gpt-6.1-sol':raise ValueError('Only the approved GPT-6.1 Sol route is enabled for this run')
    key=re.sub(r'\$\{([A-Za-z_][A-Za-z0-9_]*)\}',lambda match:os.environ.get(match[1],''),selected['api_key'])
    if not key or '${' in key:raise ValueError('Configured credential is unavailable')
    settings=selected['models'][api_model].get('settings',{})
    return selected['base_url'],key,settings,hashlib.sha256(raw).hexdigest()


def run(args):
    if not re.fullmatch(r'[a-z0-9-]{1,30}',args.label) or not 1<=args.timeout<=900:
        raise ValueError('Invalid evaluation label or timeout')
    corpus=json.loads(args.corpus.read_text())
    tasks=corpus['tasks']
    if args.tasks:tasks=[t for t in tasks if t['id'] in args.tasks.split(',')]
    if not tasks:raise ValueError('No selected evaluation tasks')
    output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    driver=Path(__file__).resolve().parent
    endpoint,key,settings,config_hash=source_config(args.config,args.model)
    nonce=secrets.token_hex(5);network='craftsman-eval-'+nonce;gateway='craftsman-eval-gateway-'+nonce
    gateway_process=None;active=None
    ledger_dir=args.ledger.resolve().parent;ledger_dir.mkdir(parents=True,exist_ok=True)
    lease=ledger_dir/'gateway-owner.json';lease_bytes=json.dumps({'gateway':gateway,'nonce':nonce}).encode()
    tokens={secrets.token_urlsafe(32):args.label+'-'+t['id'] for t in tasks}
    token_for_run={run:token for token,run in tokens.items()}
    if args.synthetic:
        prices={'input_per_million':'0','output_per_million':'0','request_fee':'0'}
        confirmed=True;cap='25.00'
    else:
        if args.prices is None:raise ValueError('Confirmed endpoint prices are required')
        priced=json.loads(args.prices.read_text())
        if priced.get('confirmed') is not True:raise ValueError('Endpoint rates are not confirmed')
        prices={key:priced[key] for key in ['input_per_million','output_per_million','request_fee']}
        confirmed=True;cap=priced['cap_usd']
    if not 0<money(cap)<=money('25.00'):raise ValueError('Evaluation cap exceeds the approved allowance')
    for price in prices.values():money(price)
    with lease.open('xb') as owner:owner.write(lease_bytes)
    bootstrap={'endpoint':endpoint,'api_key':key,'model':'gpt-6.1-sol','tokens':tokens,'prices':prices,'prices_verified':confirmed,'cap_usd':cap,'ledger':'/ledger/'+args.ledger.name}
    if args.synthetic:
        if endpoint!='http://fixture:9876/v1' or key!='fixture-only-key':raise ValueError('Synthetic route must use the fixed fixture')
        bootstrap['synthetic']=True
    report={'version':1,'model':args.model,'cap_usd':cap,'prices':prices,'pricing_source':'Operator-confirmed endpoint rate file.','image':docker('image','inspect',args.image,'--format','{{.Id}}'),
            'config_sha256':config_hash,'corpus_sha256':hashlib.sha256(args.corpus.read_bytes()).hexdigest(),'tasks':[]}
    try:
        docker('network','create','--internal',network)
        if args.synthetic:
            docker('run','--detach','--name','craftsman-eval-fixture-'+nonce,'--network',network,'--network-alias','fixture','--read-only',
                   '--mount',f'type=bind,src={driver},dst=/runner,readonly','--entrypoint','python3',args.image,'-I','/runner/fixture_provider.py')
        docker('run','--detach','--name',gateway,'--network','bridge','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges','--pids-limit','128','--memory','256m',
               '--user',f'{os.getuid()}:{os.getgid()}','--mount',f'type=bind,src={driver},dst=/runner,readonly','--mount',f'type=bind,src={ledger_dir},dst=/ledger',
               '--entrypoint','sleep',args.image,'86400')
        docker('network','connect',network,gateway)
        gateway_process=subprocess.Popen(['docker','exec','--interactive',gateway,'python3','-I','/runner/gateway.py'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        gateway_process.stdin.write(json.dumps(bootstrap).encode());gateway_process.stdin.close()
        poll=selectors.DefaultSelector();poll.register(gateway_process.stdout,selectors.EVENT_READ)
        if not poll.select(15):raise ValueError('Gateway readiness timed out')
        ready=json.loads(gateway_process.stdout.readline());poll.close()
        if ready.get('ready') is not True:raise ValueError('Gateway did not initialize')
        for index,task in enumerate(tasks):
            run_id=args.label+'-'+task['id'];case=output/run_id;case.mkdir();workspace=case/'workspace';workspace.mkdir()
            for relative,content in task['files'].items():
                target=workspace/relative
                if relative.startswith('/') or '..' in Path(relative).parts:raise ValueError('Task path escaped workspace')
                target.parent.mkdir(parents=True,exist_ok=True);target.write_text(content)
            git=['git','-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false','-c','user.name=Craftsman evaluation','-c','user.email=eval@localhost']
            subprocess.run([*git,'init','--quiet',str(workspace)],check=True,stdout=subprocess.DEVNULL)
            subprocess.run([*git,'-C',str(workspace),'add','--all'],check=True,stdout=subprocess.DEVNULL)
            subprocess.run([*git,'-C',str(workspace),'commit','--quiet','--no-gpg-sign','-m','Task baseline'],check=True,stdout=subprocess.DEVNULL)
            eval_settings=dict(settings);generation=dict(eval_settings.get('generation',{}));generation['max_output_tokens']=8192;eval_settings['generation']=generation
            config={'model':args.model,'providers':{args.model.split('/',1)[0]:{'protocol':'openai_responses','base_url':f'http://{gateway}:8765/v1','api_key':token_for_run[run_id],
                    'models':{'gpt-6.1-sol':{'settings':eval_settings}}}},'permission_mode':'manual','auto_approve':False,'voice':{'enabled':False},'gadgets':{'enabled':False},'mcp_servers':[],'lsp_servers':{}}
            active='craftsman-eval-task-'+nonce+'-'+str(index)
            streams,code,elapsed,error=capture(base_args(args.image,active,network,workspace,driver,args.arm),{'config':config,'prompt':task['prompt']},active,args.timeout)
            for channel,text in streams.items():(case/(channel+'.jsonl' if channel=='stdout' else channel+'.txt')).write_text(text)
            # Stop all agent-container processes before adding held-out tests.
            try:docker('rm','--force',active,timeout=15)
            except subprocess.CalledProcessError:pass
            active=None
            result=None
            for line in streams['stdout'].splitlines():
                try:value=json.loads(line)
                except ValueError:continue
                if value.get('kind')=='result':result=value
            captured,rejected=snapshot_files(workspace)
            modified=[];unrelated=list(rejected)
            for relative,data in captured.items():
                if relative not in task['files'] or data!=task['files'][relative].encode():modified.append(relative)
                if relative in {'Cargo.toml','.gitignore'} and data!=task['files'].get(relative,'').encode():unrelated.append(relative)
                if relative in {'test.cjs','test_public.py','tests/public.rs'} and not retained_checks(task['language'],task['files'][relative],data.decode('utf-8','strict')):unrelated.append(relative)
            for relative in task['files']:
                if relative not in captured:modified.append(relative);unrelated.append(relative)
            # Grade a separate source snapshot, with a fresh native build target.
            candidate=case/'candidate';candidate.mkdir()
            for relative,data in captured.items():
                destination=candidate/relative;destination.parent.mkdir(parents=True,exist_ok=True);destination.write_bytes(data)
            grader=case/'grader';grader.mkdir()
            scored=grade(task,candidate,grader,args.image)
            (case/'grader.txt').write_text(scored['output'])
            final=(result or {}).get('final') or {}
            stop=final.get('result',{}).get('stopReason')
            passed=scored['passed'] and not unrelated and code==0 and stop=='end_turn'
            row={'id':task['id'],'category':task['category'],'language':task['language'],'passed':passed,'grader_passed':scored['passed'],'exit':code,'stop_reason':stop,
                 'elapsed_seconds':elapsed,'modified_files':modified,'unrelated_edits':unrelated,'error':error,'approvals_once':(result or {}).get('approvals_once',0),'human_questions':(result or {}).get('human_questions',0)}
            report['tasks'].append(row);(case/'result.json').write_text(json.dumps(row,indent=2)+'\n');(output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(row),flush=True)
            ledger=json.loads(args.ledger.read_text())
            if ledger['closed'] or sum(r['charged_micros'] for r in ledger['requests'])>=int(money(cap)*1000000):break
    finally:
        if active:
            try:docker('rm','--force',active,timeout=15)
            except subprocess.CalledProcessError:pass
        try:docker('rm','--force',gateway,timeout=15)
        except subprocess.CalledProcessError:pass
        if args.synthetic:
            try:docker('rm','--force','craftsman-eval-fixture-'+nonce,timeout=15)
            except subprocess.CalledProcessError:pass
        if gateway_process:
            try:gateway_process.wait(timeout=5)
            except subprocess.TimeoutExpired:gateway_process.kill();gateway_process.wait(timeout=5)
        try:docker('network','rm',network,timeout=15)
        except subprocess.CalledProcessError:pass
        report['config_preserved']=hashlib.sha256(args.config.read_bytes()).hexdigest()==config_hash
        report['completed_tasks']=len(report['tasks']);report['passed_tasks']=sum(t['passed'] for t in report['tasks'])
        if args.ledger.exists():
            ledger=json.loads(args.ledger.read_text());rows=ledger['requests']
            report['accounted_usd']=str(sum(r['charged_micros'] for r in rows)/1000000);report['unsettled_requests']=sum(r['state']!='settled' for r in rows)
        (output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
        if lease.read_bytes()==lease_bytes:lease.unlink()
    return report


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--image',required=True);p.add_argument('--corpus',required=True,type=Path);p.add_argument('--config',required=True,type=Path);p.add_argument('--output',required=True,type=Path)
    p.add_argument('--model',default='openai/gpt-6.1-sol');p.add_argument('--tasks');p.add_argument('--timeout',type=int,default=300)
    p.add_argument('--ledger',required=True,type=Path);p.add_argument('--label',default='baseline')
    p.add_argument('--synthetic',action='store_true')
    p.add_argument('--prices',type=Path)
    p.add_argument('--arm',choices=['craftsman','codex'],default='craftsman')
    try:print(json.dumps(run(p.parse_args()),indent=2))
    except (ValueError,KeyError,subprocess.CalledProcessError,subprocess.TimeoutExpired):raise SystemExit('Evaluation setup failed; inspect the bounded task artifacts.')

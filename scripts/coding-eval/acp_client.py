"""Run the actual ACP server inside the isolated task container."""
import json
import os
from pathlib import Path
import subprocess
import sys


def run(bootstrap):
    home = Path('/home/eval');conf=home/'.craftsman';conf.mkdir(parents=True, exist_ok=True)
    config_path = conf/'config.json'
    config_path.write_text(json.dumps(bootstrap['config'])+'\n');config_path.chmod(0o600)
    env = dict(os.environ)
    env.update(HOME=str(home), CRAFTSMAN_CONFIG_DIR=str(conf))
    process = subprocess.Popen(['craftsman','acp-serve'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=sys.stderr,env=env)
    def send(value):
        process.stdin.write(json.dumps(value,separators=(',',':')).encode()+b'\n');process.stdin.flush()
    send({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':1,'clientCapabilities':{},'clientInfo':{'name':'craftsman-eval','version':'1'}}})
    session=None;final=None;approvals=0;questions=0;frames=0
    try:
        while line:=process.stdout.readline(1024*1024+1):
            if len(line)>1024*1024:raise ValueError('ACP frame exceeded evaluation bound')
            frames+=1
            if frames>10000:raise ValueError('ACP frame count exceeded evaluation bound')
            message=json.loads(line)
            print(json.dumps({'kind':'acp','message':message},ensure_ascii=False),flush=True)
            if message.get('id')==1 and 'result' in message:
                send({'jsonrpc':'2.0','id':2,'method':'session/new','params':{'cwd':'/workspace','mcpServers':[]}})
            elif message.get('id')==2 and 'result' in message:
                session=message['result']['sessionId']
                send({'jsonrpc':'2.0','id':3,'method':'session/prompt','params':{'sessionId':session,'prompt':[{'type':'text','text':bootstrap['prompt']}]}})
            elif message.get('method')=='session/request_permission':
                offered={x['optionId']:x['kind'] for x in message['params']['options']}
                option=next((k for k,v in offered.items() if v=='allow_once'),None)
                if option is None:raise ValueError('ACP did not offer a one-call approval')
                approvals+=1
                send({'jsonrpc':'2.0','id':message['id'],'result':{'outcome':{'outcome':'selected','optionId':option}}})
            elif message.get('method')=='_craftsman/session/request_input':
                questions+=1
                send({'jsonrpc':'2.0','method':'session/cancel','params':{'sessionId':session}})
                send({'jsonrpc':'2.0','id':message['id'],'error':{'code':-32000,'message':'Evaluation requires human input.'}})
            elif message.get('id')==3:
                final=message;break
            elif message.get('id') in {1,2} and 'error' in message:
                final=message;break
    finally:
        process.stdin.close()
        try:exit_code=process.wait(timeout=35)
        except subprocess.TimeoutExpired:
            process.terminate();exit_code=process.wait(timeout=5)
    print(json.dumps({'kind':'result','final':final,'agent_exit':exit_code,'approvals_once':approvals,'human_questions':questions,'frames':frames}),flush=True)
    return exit_code if exit_code else (0 if final and 'result' in final else 1)


if __name__=='__main__':
    raw=sys.stdin.buffer.read(1024*1024+1)
    if len(raw)>1024*1024:raise SystemExit('Evaluation bootstrap is too large.')
    raise SystemExit(run(json.loads(raw)))

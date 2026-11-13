"""Zero-expense HTTP fixture for actual ACP edit/test/finish runner proof."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import shlex
import os

SOURCE="""def page(items, offset, limit):
    if type(offset) is not int or type(limit) is not int or offset < 0 or limit < 0:
        raise ValueError('invalid pagination')
    return list(items[offset:offset + limit])
"""


class Handler(BaseHTTPRequestHandler):
    requests=0
    def log_message(self,*_):pass
    def do_POST(self):
        body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        index=Handler.requests;Handler.requests+=1
        names={tool.get('name') or tool.get('function',{}).get('name') for tool in body.get('tools',[]) if isinstance(tool,dict)}
        print(json.dumps({'fixture_tool_names':sorted(name for name in names if isinstance(name,str)),'request_keys':sorted(body),'tool_count':len(body.get('tools',[]))}),flush=True)
        codex=os.environ.get('CRAFTSMAN_FIXTURE_ARM')=='codex' or 'exec_command' in names
        if codex and index<2:
            command=('python3 -c '+shlex.quote("from pathlib import Path; Path('toolkit.py').write_text("+repr(SOURCE)+")")) if index==0 else 'python3 -B test_public.py && test ! -e /driver/tasks.json && test ! -e /driver/check_corpus.py'
            item={'type':'function_call','id':'fixture-command-'+str(index),'call_id':'fixture-command-'+str(index),'name':'exec_command','arguments':json.dumps({'cmd':command})}
        elif index==0:
            item={'type':'function_call','id':'fixture-write','call_id':'fixture-write','name':'edit_file','arguments':json.dumps({'file_path':'toolkit.py','old_string':'def page(items, offset, limit):\n    return items[offset:limit]\n','new_string':SOURCE})}
        elif index==1:
            item={'type':'function_call','id':'fixture-test','call_id':'fixture-test','name':'execute','arguments':json.dumps({'command':'python3 -B test_public.py && test ! -e /driver/tasks.json && test ! -e /driver/check_corpus.py && test ! -e /driver/make_corpus.py && python3 -c "from pathlib import Path; assert \'fixture-only-key\' not in Path(\'/home/eval/.craftsman/config.json\').read_text()"'})}
        else:
            item={'type':'message','id':'fixture-done','role':'assistant','phase':'final','content':[{'type':'output_text','text':'Fixed pagination and ran the public check.'}]}
        event={'type':'response.completed','response':{'id':'fixture-'+str(index),'status':'completed','incomplete_details':None,'output':[item],'usage':{'input_tokens':100,'output_tokens':50,'total_tokens':150}}}
        added={**item,'status':'in_progress'}
        if item['type']=='function_call':added['arguments']=''
        else:added['content']=[]
        events=[{'type':'response.created','response':{'id':'fixture-'+str(index),'status':'in_progress','model':'gpt-6.1-sol'}},
                {'type':'response.output_item.added','output_index':0,'item':added}]
        if item['type']=='function_call':
            events.extend([{'type':'response.function_call_arguments.delta','item_id':item['id'],'output_index':0,'delta':item['arguments']},
                           {'type':'response.function_call_arguments.done','item_id':item['id'],'output_index':0,'arguments':item['arguments']}])
        elif not codex:
            events.append({'type':'response.content_part.added','item_id':item['id'],'output_index':0,'content_index':0,'part':{'type':'output_text','text':''}})
            events.append({'type':'response.output_text.delta','item_id':item['id'],'output_index':0,'content_index':0,'delta':item['content'][0]['text']})
            events.append({'type':'response.content_part.done','item_id':item['id'],'output_index':0,'content_index':0,'part':item['content'][0]})
        events.extend([{'type':'response.output_item.done','output_index':0,'item':{**item,'status':'completed'}},event])
        data=''.join('data: '+json.dumps(value)+'\n\n' for value in events).encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)


if __name__=='__main__':ThreadingHTTPServer(('0.0.0.0',9876),Handler).serve_forever()

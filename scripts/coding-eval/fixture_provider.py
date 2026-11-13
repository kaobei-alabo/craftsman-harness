"""Zero-expense HTTP fixture for actual ACP edit/test/finish runner proof."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json

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
        if index==0:
            item={'type':'function_call','id':'fixture-write','call_id':'fixture-write','name':'edit_file','arguments':json.dumps({'file_path':'toolkit.py','old_string':'def page(items, offset, limit):\n    return items[offset:limit]\n','new_string':SOURCE})}
        elif index==1:
            item={'type':'function_call','id':'fixture-test','call_id':'fixture-test','name':'execute','arguments':json.dumps({'command':'python3 -B test_public.py && test ! -e /driver/tasks.json && test ! -e /driver/check_corpus.py && test ! -e /driver/make_corpus.py && python3 -c "from pathlib import Path; assert \'fixture-only-key\' not in Path(\'/home/eval/.craftsman/config.json\').read_text()"'})}
        else:
            item={'type':'message','id':'fixture-done','role':'assistant','content':[{'type':'output_text','text':'Fixed pagination and ran the public check.'}]}
        event={'type':'response.completed','response':{'id':'fixture-'+str(index),'status':'completed','incomplete_details':None,'output':[item],'usage':{'input_tokens':100,'output_tokens':50}}}
        data=('data: '+json.dumps(event)+'\n\n').encode()
        self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)


if __name__=='__main__':ThreadingHTTPServer(('0.0.0.0',9876),Handler).serve_forever()

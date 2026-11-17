"""A trusted evaluation gateway: real credentials enter only on its stdin."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import sys
from pathlib import Path
import urllib.error
import urllib.parse
import urllib.request
sys.path.insert(0, str(Path(__file__).resolve().parent))
from budget import Budget, Refused, checked_request
from preflight_continuation import read_json
import re
from gateway_observations import GatewayObservations, ObservationFailure, refusal_reason


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class Usage:
    def __init__(self):
        self.pending = bytearray()
        self.usage = None
        self.total = 0
        self.events = {}
        self.terminal_event = None
        self.reason = None

    def feed(self, chunk):
        self.total += len(chunk)
        if self.total > 8 * 1024 * 1024:
            raise Refused('Evaluation response exceeded its bound.')
        self.pending.extend(chunk)
        if len(self.pending) > 1024 * 1024:
            raise Refused('Evaluation event exceeded its bound.')
        while b'\n' in self.pending:
            line, _, rest = self.pending.partition(b'\n');self.pending = bytearray(rest)
            if line.startswith(b'data: '):
                try:event = json.loads(line[6:])
                except (ValueError, UnicodeError):continue
                if not isinstance(event,dict):continue
                kind=event.get('type')
                if not isinstance(kind,str):continue
                if kind in {'response.completed','response.incomplete','response.failed','error'}:
                    self.terminal_event=kind
                    response=event.get('response')
                    response=response if isinstance(response,dict) else {}
                    details=response.get('incomplete_details')
                    details=details if isinstance(details,dict) else {}
                    reason=details.get('reason')
                    if isinstance(reason,str) and reason in {'max_output_tokens','content_filter'}:self.reason=reason
                if kind in {'response.completed','response.incomplete','response.failed','error','response.output_text.delta','response.created'}:
                    self.events[kind]=self.events.get(kind,0)+1
                if event.get('type') in {'response.completed','response.incomplete'}:
                    response=event.get('response')
                    self.usage = response.get('usage') if isinstance(response,dict) else None


def checked_budget(config):
    expected=config.get('expected_ledger_sha256')
    if expected is not None:
        if not isinstance(expected,str) or not re.fullmatch(r'[0-9a-f]{64}',expected):
            raise Refused('Invalid continuation ledger binding.')
        _,actual=read_json(Path(config['ledger']),4*1024*1024)
        if actual!=expected:
            raise Refused('Continuation ledger changed before gateway admission.')
    return Budget(config['ledger'], config['prices'], config['cap_usd'], config.get('prices_verified'))


def serve(config, synthetic_listen=None):
    endpoint = config['endpoint']
    parsed = urllib.parse.urlsplit(endpoint)
    synthetic = config.get('synthetic') is True
    if parsed.username or parsed.password or parsed.query or parsed.fragment or (parsed.scheme != 'https' and not (synthetic and parsed.hostname in {'127.0.0.1','localhost','fixture'})):
        raise Refused('Invalid evaluation endpoint.')
    budget = checked_budget(config)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    tokens = config['tokens']
    if not isinstance(tokens, dict) or not 1 <= len(tokens) <= 256:
        raise Refused('Invalid evaluation run tokens.')
    observations = None
    if config.get('private_observations') is not None and type(config['private_observations']) is not bool:
        raise Refused('Invalid private observation selection.')
    if config.get('private_observations') is True:
        if not synthetic:
            raise Refused('Private observations require the isolated synthetic driver.')
        observations = GatewayObservations(sys.stdout)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def log_message(self, *_):pass

        def fixed(self, status, message):
            data=json.dumps({'error':{'message':message}}).encode()
            self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)

        def do_GET(self):
            self.fixed(404,'Evaluation route is not available.')

        def do_POST(self):
            ticket=None;usage=Usage();status=None;run=None
            try:
                auth=self.headers.get('Authorization','')
                run=tokens.get(auth.removeprefix('Bearer ')) if auth.startswith('Bearer ') else None
                if run is None or self.path != '/v1/responses' or self.headers.get('Transfer-Encoding') is not None:
                    self.fixed(403,'Evaluation request is not allowed.');return
                length=self.headers.get('Content-Length','')
                if not length.isdigit() or not 1<=int(length)<=256*1024:
                    self.fixed(400,'Invalid evaluation request size.');return
                if observations is not None:observations.record('attempted',run)
                self.connection.settimeout(120)
                raw=self.rfile.read(int(length))
                if len(raw)!=int(length):raise Refused('Incomplete evaluation request.')
                body,input_upper,output_max=checked_request(raw,config['model'])
                ticket=budget.reserve(run,input_upper,output_max)
                if observations is not None:observations.record('admitted',run,ticket.ordinal)
                request=urllib.request.Request(endpoint.rstrip('/')+'/responses',data=body,
                    headers={'Authorization':'Bearer '+config['api_key'],'Content-Type':'application/json','Accept':'text/event-stream'},method='POST')
                with opener.open(request,timeout=120) as response:
                    status=response.status
                    if observations is not None:observations.record('provider_headers',run,ticket.ordinal,status)
                    content_type=response.headers.get('Content-Type','')
                    if not content_type.startswith('text/event-stream'):
                        raise Refused('Evaluation provider did not return an event stream.')
                    self.send_response(status);self.send_header('Content-Type','text/event-stream');self.send_header('Connection','close');self.end_headers()
                    self.close_connection=True;connected=True
                    while chunk:=response.read1(16384):
                        usage.feed(chunk)
                        if connected:
                            try:self.wfile.write(chunk);self.wfile.flush()
                            except (BrokenPipeError,ConnectionResetError):connected=False
                    if observations is not None:observations.record('provider_eof',run,ticket.ordinal,status,terminal=usage.terminal_event)
            except urllib.error.HTTPError as error:
                status=error.code
                self.fixed(502,'Evaluation provider refused the request.')
            except Refused as error:
                if observations is not None and ticket is None and run is not None:
                    try:observations.record('refused',run,reason=refusal_reason(error))
                    except ObservationFailure:pass
                if ticket is None:self.fixed(402,str(error))
                else:self.close_connection=True
            except ObservationFailure:
                self.close_connection=True
            except (OSError, ValueError, urllib.error.URLError):
                if observations is not None and run is not None:
                    try:observations.record('transport_error',run,ticket.ordinal if ticket is not None else None,status)
                    except ObservationFailure:pass
                self.close_connection=True
                if ticket is None:
                    try:self.fixed(502,'Evaluation transport failed.')
                    except OSError:pass
            finally:
                if ticket is not None:
                    try:
                        budget.settle(ticket,usage.usage,status,{'terminal_event':usage.terminal_event,'reason':usage.reason,'events':usage.events})
                        if observations is not None:
                            state=budget.data['requests'][ticket.ordinal]['state']
                            observations.record('settled' if state=='settled' else 'unknown_usage',run,ticket.ordinal,status)
                    except (Refused,ObservationFailure):pass

    if synthetic_listen is not None and not synthetic:
        raise Refused('Synthetic listen selection refused.')
    server=ThreadingHTTPServer(synthetic_listen or ('0.0.0.0',8765),Handler)
    ready={'ready':True,'model':config['model'],'cap_usd':config['cap_usd']}
    if synthetic_listen is not None:ready['synthetic_bound_port']=server.server_address[1]
    print(json.dumps(ready),flush=True)
    server.serve_forever(poll_interval=0.1)


if __name__ == '__main__':
    # Bootstrap is provided to a separate trusted process, never an agent env.
    raw=sys.stdin.buffer.read(1024*1024+1)
    if len(raw)>1024*1024:raise SystemExit('Evaluation bootstrap exceeded its bound.')
    try:serve(json.loads(raw))
    except (Refused,KeyError,ValueError,TypeError):raise SystemExit('Evaluation gateway initialization refused.')

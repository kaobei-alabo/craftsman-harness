"""Evaluation-only admission and accounting; not the app's public headless meter."""
from decimal import Decimal, InvalidOperation, ROUND_CEILING
import json
import os
from pathlib import Path
import re
import threading


class Refused(Exception):
    pass


def money(value):
    if not isinstance(value, str) or not re.fullmatch(r'(?:0|[1-9][0-9]{0,4})(?:\.[0-9]{1,6})?', value):
        raise Refused('Invalid evaluation pricing.')
    try:
        return Decimal(value)
    except InvalidOperation:
        raise Refused('Invalid evaluation pricing.') from None


def integer(value, maximum=2**63-1):
    if type(value) is not int or not 0 <= value <= maximum:
        raise Refused('Invalid evaluation accounting.')
    return value


class Budget:
    def __init__(self, path, prices, limit='25.00', verified=False):
        if verified is not True:
            raise Refused('Endpoint pricing must be confirmed before live calls.')
        self.input_price = money(prices['input_per_million'])
        self.output_price = money(prices['output_per_million'])
        self.fee = money(prices.get('request_fee', '0'))
        cap = money(limit)
        if not 0 < cap <= Decimal('25'):
            raise Refused('Evaluation cap exceeds the approved allowance.')
        self.cap = int(cap * 1000000)
        self.path = Path(path)
        self.lock = threading.Lock()
        self.closed = False
        binding = {'limit': limit, 'prices': prices}
        if self.path.exists():
            if self.path.stat().st_size > 4 * 1024 * 1024:
                raise Refused('Invalid evaluation ledger.')
            self.data = json.loads(self.path.read_text())
            if self.data.get('version') != 1 or self.data.get('binding') != binding or self.data.get('closed') is not False:
                raise Refused('Invalid evaluation ledger.')
            rows = self.data.get('requests')
            if not isinstance(rows, list) or len(rows) > 10000:
                raise Refused('Invalid evaluation ledger.')
            for i, row in enumerate(rows):
                if row['ordinal'] != i or row['state'] not in {'reserved', 'settled', 'unsettled'}:
                    raise Refused('Invalid evaluation ledger.')
                integer(row['charged_micros']);integer(row['reserved_micros'])
                if row['charged_micros'] > row['reserved_micros']:
                    raise Refused('Invalid evaluation ledger.')
            if sum(row['charged_micros'] for row in rows) > self.cap:
                raise Refused('Invalid evaluation ledger.')
        else:
            self.data = {'version': 1, 'binding': binding, 'closed': False, 'requests': []}
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with self.path.open('x') as output:
                os.chmod(self.path, 0o600)
                json.dump(self.data, output)
                output.flush();os.fsync(output.fileno())

    def cost(self, input_tokens, output_tokens):
        integer(input_tokens);integer(output_tokens)
        return int((input_tokens * self.input_price + output_tokens * self.output_price + self.fee * 1000000).to_integral_value(rounding=ROUND_CEILING))

    def save(self):
        temporary = self.path.with_name(self.path.name + '.next')
        try:
            with temporary.open('x') as output:
                os.chmod(temporary, 0o600)
                json.dump(self.data, output)
                output.flush();os.fsync(output.fileno())
            os.replace(temporary, self.path)
        except (OSError, ValueError):
            self.closed = True
            self.data['closed'] = True
            raise Refused('Evaluation ledger save failed; further calls refused.') from None

    def reserve(self, run_id, input_upper, output_max, per_run=20):
        integer(input_upper, 270000);integer(output_max, 8192)
        if output_max < 1 or not isinstance(run_id, str) or not re.fullmatch(r'[a-z0-9-]{1,100}', run_id):
            raise Refused('Invalid evaluation request.')
        charge = self.cost(input_upper, output_max)
        with self.lock:
            rows = self.data['requests']
            if self.closed or len(rows) >= 10000:
                raise Refused('Evaluation accounting is closed.')
            if sum(r['run'] == run_id for r in rows) >= per_run:
                raise Refused('Evaluation run reached its request limit.')
            if sum(r['charged_micros'] for r in rows) + charge > self.cap:
                raise Refused('Evaluation reached its spending cap.')
            ordinal = len(rows)
            rows.append({'ordinal': ordinal, 'run': run_id, 'state': 'reserved', 'input_upper': input_upper,
                         'output_max': output_max, 'reserved_micros': charge, 'charged_micros': charge,
                         'input_tokens': None, 'output_tokens': None, 'http_status': None})
            self.save()
            return Ticket(self, ordinal)

    def settle(self, ticket, usage, status, observation=None):
        with self.lock:
            if ticket.owner is not self or ticket.finished:
                self.closed = True
                self.data['closed'] = True
                self.save()
                raise Refused('Evaluation accounting identity failed.')
            ticket.finished = True
            row = self.data['requests'][ticket.ordinal]
            row['http_status'] = status
            if observation is not None:row['response_observation']=observation
            try:
                if usage is None:
                    raise Refused('Missing provider usage.')
                input_tokens = integer(usage['input_tokens'])
                output_tokens = integer(usage['output_tokens'])
                row.update(input_tokens=input_tokens, output_tokens=output_tokens)
                if input_tokens > row['input_upper'] or output_tokens > row['output_max']:
                    row['charged_micros'] = max(row['reserved_micros'], self.cost(input_tokens, output_tokens))
                    raise Refused('Provider usage exceeded its reservation.')
                row.update(state='settled', input_tokens=input_tokens, output_tokens=output_tokens,
                           charged_micros=self.cost(input_tokens, output_tokens))
            except (Refused, KeyError, TypeError):
                row['state'] = 'unsettled'
                if usage is not None:
                    self.closed = True;self.data['closed'] = True
            self.save()

    def snapshot(self):
        with self.lock:
            rows = self.data['requests']
            return {'cap_usd': str(Decimal(self.cap) / 1000000),
                    'accounted_usd': str(Decimal(sum(r['charged_micros'] for r in rows)) / 1000000),
                    'requests': len(rows), 'settled': sum(r['state'] == 'settled' for r in rows),
                    'unsettled': sum(r['state'] != 'settled' for r in rows), 'closed': self.closed,
                    'input_tokens': sum(r['input_tokens'] for r in rows if r['input_tokens'] is not None),
                    'output_tokens': sum(r['output_tokens'] for r in rows if r['output_tokens'] is not None)}


class Ticket:
    def __init__(self, owner, ordinal):
        self.owner = owner
        self.ordinal = ordinal
        self.finished = False


def checked_request(raw, model, maximum=8192):
    if len(raw) > 256 * 1024:
        raise Refused('Evaluation request is too large.')
    try:
        body = json.loads(raw)
    except (ValueError, UnicodeError, RecursionError):
        raise Refused('Invalid evaluation request.') from None
    if not isinstance(body, dict) or body.get('model') != model:
        raise Refused('Evaluation model is not allowed.')
    tier=body.get('service_tier','default')
    if any(key in body for key in ['previous_response_id', 'conversation', 'background']) or not isinstance(tier,str) or tier not in {'default', 'auto'}:
        raise Refused('Unsupported evaluation request mode.')
    tools = body.get('tools', [])
    if not isinstance(tools, list) or len(tools) > 512:
        raise Refused('Invalid evaluation tool definitions.')
    for tool in tools:
        if not isinstance(tool, dict) or not isinstance(tool.get('type'),str) or tool.get('type') not in {'function', 'custom'}:
            raise Refused('Hosted evaluation tools are not allowed.')
    def inspect(value, depth=0):
        if depth > 64:
            raise Refused('Evaluation request is too deep.')
        if isinstance(value, dict):
            if isinstance(value.get('type'),str) and value.get('type') in {'input_image','input_audio','input_file','item_reference'}:
                raise Refused('Only self-contained text evaluation requests are allowed.')
            for child in value.values():inspect(child, depth+1)
        elif isinstance(value, list):
            for child in value:inspect(child, depth+1)
    inspect(body)
    requested = body.get('max_output_tokens', maximum)
    if type(requested) is not int or not 1 <= requested <= maximum:
        raise Refused('Evaluation output ceiling is not allowed.')
    body['max_output_tokens'] = requested
    encoded = json.dumps(body, ensure_ascii=False, separators=(',', ':')).encode()
    # Text requests use a byte-level upper allowance plus framing headroom;
    # long-context and server-side conversation references are excluded.
    upper = len(encoded) + 8192
    if upper > 270000:
        raise Refused('Evaluation input reservation is too large.')
    return encoded, upper, requested

"""Offline financial-admission and request-boundary regressions."""
import concurrent.futures
import json
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parent))
from budget import Budget, Refused, checked_request
from gateway import Usage


PRICES={'input_per_million':'2','output_per_million':'10','request_fee':'0'}


class Cases(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name)
    def tearDown(self):self.temp.cleanup()
    def budget(self,**kwargs):return Budget(self.root/'ledger.json',PRICES,verified=True,**kwargs)
    def test_unconfirmed_pricing_refuses_without_ledger(self):
        with self.assertRaises(Refused):Budget(self.root/'ledger.json',PRICES)
        self.assertFalse((self.root/'ledger.json').exists())
    def test_cap_cannot_exceed_authorization(self):
        for cap in ['25.01','0','-1','Infinity','NaN']:
            with self.subTest(cap=cap),self.assertRaises(Refused):self.budget(limit=cap)
    def test_reservation_exact_boundary(self):
        b=self.budget(limit='0.10');b.reserve('first',0,8192)
        with self.assertRaises(Refused):b.reserve('second',0,8192)
        self.assertEqual(b.snapshot()['requests'],1);self.assertEqual(b.snapshot()['accounted_usd'],'0.08192')
    def test_actual_usage_releases_only_confirmed_unused_reservation(self):
        b=self.budget();t=b.reserve('first',1000,1000);b.settle(t,{'input_tokens':100,'output_tokens':20},200)
        self.assertEqual(b.snapshot()['accounted_usd'],'0.0004');self.assertEqual(b.snapshot()['settled'],1)
    def test_unknown_usage_remains_reserved(self):
        b=self.budget();t=b.reserve('first',1000,1000);b.settle(t,None,502)
        self.assertEqual(b.snapshot()['accounted_usd'],'0.012');self.assertEqual(b.snapshot()['unsettled'],1)
        self.assertEqual(json.loads(b.path.read_text())['requests'][0]['input_tokens'],None)
    def test_invalid_usage_closes_new_admission(self):
        b=self.budget();t=b.reserve('first',1000,1000);b.settle(t,{'input_tokens':1001,'output_tokens':1},200)
        with self.assertRaises(Refused):b.reserve('later',10,10)
        self.assertTrue(b.snapshot()['closed'])
        self.assertEqual(b.snapshot()['input_tokens'],1001)
        self.assertEqual(b.snapshot()['output_tokens'],1)
    def test_reused_ticket_closes_accounting(self):
        b=self.budget();t=b.reserve('first',1,1);b.settle(t,{'input_tokens':1,'output_tokens':1},200)
        with self.assertRaises(Refused):b.settle(t,{'input_tokens':1,'output_tokens':1},200)
    def test_reopen_retains_unsettled_cost(self):
        b=self.budget();b.reserve('first',10,10);reopened=self.budget()
        self.assertEqual(reopened.snapshot()['accounted_usd'],'0.00012');self.assertEqual(reopened.snapshot()['requests'],1)
    def test_reopen_cannot_change_rates(self):
        self.budget()
        with self.assertRaises(Refused):Budget(self.root/'ledger.json',{**PRICES,'output_per_million':'1'},verified=True)
    def test_concurrent_requests_share_one_cap(self):
        b=self.budget(limit='0.10')
        def attempt(i):
            try:b.reserve('r-'+str(i),0,1000);return True
            except Refused:return False
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:results=list(pool.map(attempt,range(40)))
        self.assertEqual(sum(results),10);self.assertEqual(b.snapshot()['accounted_usd'],'0.1')
    def test_request_limit_covers_same_run(self):
        b=self.budget();b.reserve('same',1,1,per_run=1)
        with self.assertRaises(Refused):b.reserve('same',1,1,per_run=1)
    def test_save_failure_refuses_more_calls(self):
        b=self.budget();b.path.with_name(b.path.name+'.next').write_text('foreign')
        with self.assertRaises(Refused):b.reserve('first',1,1)
        with self.assertRaises(Refused):b.reserve('later',1,1)
        self.assertEqual(b.path.with_name(b.path.name+'.next').read_text(),'foreign')
    def test_request_model_and_output_cap(self):
        body,upper,ceiling=checked_request(b'{"model":"gpt-6.1-sol","input":"hi"}','gpt-6.1-sol')
        self.assertEqual(json.loads(body)['max_output_tokens'],8192);self.assertGreater(upper,len(body));self.assertEqual(ceiling,8192)
        for value in [0,-1,8193,True,'8192']:
            with self.subTest(value=value),self.assertRaises(Refused):checked_request(json.dumps({'model':'gpt-6.1-sol','max_output_tokens':value}).encode(),'gpt-6.1-sol')
    def test_server_references_and_hosted_tools_refuse(self):
        for extra in [{'previous_response_id':'r'},{'conversation':'c'},{'tools':[{'type':'web_search'}]},{'input':[{'type':'input_image','image_url':'https://example.org'}]},{'model':'different'},{'tools':None},{'service_tier':'priority'}]:
            with self.subTest(extra=extra),self.assertRaises(Refused):checked_request(json.dumps({'model':'gpt-6.1-sol',**extra}).encode(),'gpt-6.1-sol')
    def test_split_usage_stream(self):
        u=Usage();payload=b'data: {"type":"response.completed","response":{"usage":{"input_tokens":20,"output_tokens":7}}}\n\n'
        for byte in payload:u.feed(bytes([byte]))
        self.assertEqual(u.usage,{'input_tokens':20,'output_tokens':7})
    def test_text_cannot_forge_outer_usage(self):
        u=Usage();u.feed(b'data: {"type":"response.output_text.delta","delta":"response.completed"}\n\n')
        self.assertIsNone(u.usage)
    def test_boolean_usage_is_not_zero(self):
        b=self.budget();t=b.reserve('first',10,10);b.settle(t,{'input_tokens':False,'output_tokens':0},200)
        self.assertTrue(b.snapshot()['closed']);self.assertEqual(b.snapshot()['unsettled'],1)
    def test_completed_null_details_retains_usage_and_forwardable_event(self):
        u=Usage();payload={'type':'response.completed','response':{'incomplete_details':None,'usage':{'input_tokens':40,'output_tokens':11}}}
        u.feed(('data: '+json.dumps(payload)+'\n\n').encode())
        self.assertEqual(u.usage,{'input_tokens':40,'output_tokens':11});self.assertEqual(u.terminal_event,'response.completed');self.assertIsNone(u.reason)
        self.assertEqual(u.events['response.completed'],1)
    def test_optional_diagnostics_do_not_interrupt_valid_usage(self):
        for details in [None,{},'unknown',{'reason':None},{'reason':'not-public'},{'reason':[]},{'reason':{}},{'reason':1}]:
            with self.subTest(details=details):
                u=Usage();u.feed(('data: '+json.dumps({'type':'response.completed','response':{'incomplete_details':details,'usage':{'input_tokens':1,'output_tokens':1}}})+'\n\n').encode())
                self.assertEqual(u.usage,{'input_tokens':1,'output_tokens':1});self.assertIsNone(u.reason)
    def test_incomplete_reason_is_a_fixed_allowed_value(self):
        u=Usage();u.feed(b'data: {"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":2,"output_tokens":8192}}}\n\n')
        self.assertEqual(u.reason,'max_output_tokens');self.assertEqual(u.usage['output_tokens'],8192)
    def test_nullable_schema_types_are_not_multimedia_inputs(self):
        request={'model':'gpt-6.1-sol','tools':[{'type':'function','name':'x','parameters':{'type':'object','properties':{'x':{'type':['string','null']}}}}]}
        encoded,_,_=checked_request(json.dumps(request).encode(),'gpt-6.1-sol')
        self.assertEqual(json.loads(encoded)['tools'],request['tools'])
    def test_bad_type_values_refuse_before_admission(self):
        for extra in [{'service_tier':[]},{'tools':[{'type':[]}]}]:
            with self.subTest(extra=extra),self.assertRaises(Refused):checked_request(json.dumps({'model':'gpt-6.1-sol',**extra}).encode(),'gpt-6.1-sol')


if __name__=='__main__':unittest.main()

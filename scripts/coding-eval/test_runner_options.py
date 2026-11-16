"""Exact cohort selection and safe explicit experimental configuration."""
import sys
from pathlib import Path
import unittest
import tempfile
import json
import hashlib
from types import SimpleNamespace
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parent))
from run_suite import selected_tasks, agent_configuration, load_feature_options, credential_free_settings, continuation_binding, run, effective_settings_record
from gateway import checked_budget
import test_preflight_continuation as preflight_tests


class Cases(unittest.TestCase):
    def test_recorded_model_settings_come_from_emitted_config_without_credentials(self):
        original={'generation':{'max_output_tokens':123},'reasoning':{'type':'effort','value':'high'}}
        options={'context':{'enabled':True,'max_bytes':12288}}
        config=agent_configuration('openai/gpt-6.1-sol','private-route','synthetic-sensitive-key',original,options,'xhigh')
        recorded=effective_settings_record(config)
        self.assertEqual(recorded['effective_model_settings'],{'generation':{'max_output_tokens':8192},'reasoning':{'type':'effort','value':'xhigh'}})
        self.assertEqual(recorded['effective_model_settings_sha256'],'2689e991578f60a236cada705d84bef5590d537f873979590032821ba0b17019')
        serialized=json.dumps(recorded)
        self.assertNotIn('synthetic-sensitive-key',serialized)
        self.assertNotIn('private-route',serialized)
        self.assertNotIn('providers',serialized)
        self.assertNotIn('context',serialized)
        self.assertEqual(original['generation']['max_output_tokens'],123)
        self.assertEqual(original['reasoning']['value'],'high')
        alternate=agent_configuration('other/gpt-6.1-sol','another-route','different-temporary-key',original,{},'xhigh')
        self.assertEqual(effective_settings_record(alternate),recorded)
        recorded['effective_model_settings']['generation']['max_output_tokens']=1
        self.assertEqual(config['providers']['openai']['models']['gpt-6.1-sol']['settings']['generation']['max_output_tokens'],8192)
        alternate['providers']['other']['models']['gpt-6.1-sol']['settings']['generation']['temperature']=0.3
        self.assertNotEqual(effective_settings_record(alternate)['effective_model_settings_sha256'],effective_settings_record(config)['effective_model_settings_sha256'])

    def test_real_full_suite_forecast_cannot_start_live_continuation_effects(self):
        fixture=preflight_tests.Cases('test_valid_preflight_retains_every_original_row_and_has_no_live_result')
        fixture.setUp()
        try:
            fixture.manifest['cohort']='full_suite_forecast'
            fixture.manifest['tasks']=[task['id'] for task in fixture.corpus['tasks']]
            fixture.refresh()
            proof=fixture.run_preflight()
            self.assertTrue(proof['offline_checks_passed'])
            self.assertEqual(proof['cohort'],'full_suite_forecast')
            self.assertFalse(proof['is_live_authorization'])
            options=fixture.manifest['arms']['candidate']['options']
            option_path=fixture.root/'reviewed-options.json'
            option_path.write_text(json.dumps(options))
            args=SimpleNamespace(label='forecast-guard',timeout=300,corpus=fixture.corpus_path,
                tasks=','.join(fixture.manifest['tasks']),feature_options=option_path,effort='xhigh',
                synthetic=False,arm='craftsman',output=fixture.root/'must-not-create-output',
                config=fixture.root/'must-not-read-user-config.json',model='openai/gpt-6.1-sol',
                ledger=fixture.ledger_path,image=fixture.builds['candidate']['image_id'],prices=None,
                continuation_manifest=fixture.manifest_path,build_records=fixture.builds_path,
                ledger_checkpoint=fixture.checkpoint_path,continuation_arm='candidate')
            original_ledger=fixture.ledger_path.read_bytes()
            with patch('run_suite.source_config',side_effect=AssertionError('No configuration read is allowed')) as config_read,patch('run_suite.docker',side_effect=AssertionError('No Docker effect is allowed')) as docker_call:
                with self.assertRaisesRegex(ValueError,'selected_pair execution cohort'):
                    run(args)
                config_read.assert_not_called()
                docker_call.assert_not_called()
            self.assertFalse(args.output.exists())
            self.assertFalse((fixture.ledger_path.parent/'gateway-owner.json').exists())
            self.assertEqual(fixture.ledger_path.read_bytes(),original_ledger)
            # The same real bindings remain usable for the approved selected
            # pair shape; a forecast is not rejected merely as malformed JSON.
            fixture.manifest['cohort']='selected_pair'
            fixture.manifest['tasks']=['rs-report-skips']
            fixture.refresh()
            selected=[task for task in fixture.corpus['tasks'] if task['id']=='rs-report-skips']
            binding=continuation_binding(args,selected,options,'xhigh')
            self.assertEqual(binding['cohort'],'selected_pair')
            self.assertFalse(binding['is_live_authorization'])
        finally:
            fixture.tearDown()

    def test_live_feature_continuation_requires_binding_before_effects(self):
        args=SimpleNamespace(synthetic=False,continuation_manifest=None)
        with self.assertRaises(ValueError):continuation_binding(args,[{'id':'one'}],{'context':{'enabled':True}},'xhigh')

    def test_failed_image_inspection_does_not_leave_an_exclusive_gateway_lease(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            corpus=root/'corpus.json';corpus.write_text(json.dumps({'tasks':[{'id':'one','files':{},'prompt':'fixture'}]}))
            args=SimpleNamespace(label='fixture',timeout=300,corpus=corpus,tasks=None,feature_options=None,effort=None,
                synthetic=True,arm='craftsman',output=root/'output',config=root/'unused-user-config.json',model='openai/gpt-6.1-sol',ledger=root/'ledger/ledger.json',image='missing',prices=None)
            with patch('run_suite.source_config',return_value=('http://fixture:9876/v1','fixture-only-key',{},'f'*64)),patch('run_suite.docker',side_effect=ValueError('image unavailable')):
                with self.assertRaises(ValueError):run(args)
            self.assertFalse((args.ledger.parent/'gateway-owner.json').exists())
            self.assertFalse(args.ledger.exists())

    def test_gateway_requires_the_existing_bound_ledger_before_budget_construction(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'ledger.json'
            config={'ledger':str(path),'expected_ledger_sha256':'a'*64,'prices':{},'cap_usd':'25.00','prices_verified':True}
            with self.assertRaises(ValueError):checked_budget(config)
            self.assertFalse(path.exists())
            path.write_text('{}')
            with self.assertRaises(Exception):checked_budget(config)
            self.assertEqual(path.read_text(),'{}')
    def test_real_options_file_is_loaded_with_its_exact_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'experiment-options.json'
            raw=json.dumps({'context':{'enabled':True,'max_bytes':12288}}).encode()
            path.write_bytes(raw)
            options,digest=load_feature_options(path)
            self.assertEqual(options,{'context':{'enabled':True,'max_bytes':12288}})
            self.assertEqual(digest,hashlib.sha256(raw).hexdigest())
            self.assertEqual(load_feature_options(None),({},None))
            path.write_text(json.dumps({'mcp_servers':[]}))
            with self.assertRaises(ValueError):load_feature_options(path)

    def test_unknown_id_cannot_silently_shrink_a_cohort(self):
        tasks=[{'id':'one'},{'id':'two'}]
        with self.assertRaises(ValueError):selected_tasks(tasks,'one,missing')
        with self.assertRaises(ValueError):selected_tasks(tasks,'one,one')
        with self.assertRaises(ValueError):selected_tasks(tasks,'')
        self.assertEqual(selected_tasks(tasks,'two,one'),tasks)

    def test_absent_options_keep_runtime_services_disabled(self):
        config=agent_configuration('openai/gpt-6.1-sol','fixture','fixture-key',{}, {})
        self.assertNotIn('context',config)
        self.assertNotIn('failure_control',config)
        self.assertEqual(config['mcp_servers'],[])
        self.assertEqual(config['lsp_servers'],{})
        self.assertFalse(config['voice']['enabled'])
        self.assertFalse(config['gadgets']['enabled'])
        self.assertEqual(config['permission_mode'],'manual')

    def test_exact_feature_fields_forward_without_changing_user_settings(self):
        settings={'reasoning':{'type':'effort','value':'high'},'generation':{'temperature':0.3}}
        options={'context':{'enabled':True,'max_bytes':12288},'failure_control':{'repeat_limit':5,'test_review_signals':True}}
        config=agent_configuration('openai/gpt-6.1-sol','fixture','fixture-key',settings,options,'xhigh')
        self.assertEqual(config['context'],options['context'])
        self.assertEqual(config['failure_control'],options['failure_control'])
        self.assertEqual(settings['reasoning']['value'],'high')
        model=config['providers']['openai']['models']['gpt-6.1-sol']['settings']
        self.assertEqual(model['reasoning'],{'type':'effort','value':'xhigh'})
        self.assertEqual(model['generation']['max_output_tokens'],8192)
        self.assertEqual(model['generation']['temperature'],0.3)

    def test_null_reasoning_uses_the_actual_named_effort_schema(self):
        config=agent_configuration('openai/gpt-6.1-sol','fixture','fixture-key',{'reasoning':None},{},'xhigh')
        settings=config['providers']['openai']['models']['gpt-6.1-sol']['settings']
        self.assertEqual(settings['reasoning'],{'type':'effort','value':'xhigh'})
        for settings in [{'api_key':'private-marker'},{'headers':{'Authorization':'private-marker'}},{'generation':{'headers':{}}}]:
            with self.assertRaises(ValueError):credential_free_settings(settings)

    def test_arbitrary_services_permissions_and_effort_are_refused(self):
        for fields in [{'mcp_servers':[]},{'permission_mode':'bypass-permissions'},{'context':{'enabled':True,'endpoint':'secret'}}]:
            with self.assertRaises(ValueError):agent_configuration('openai/gpt-6.1-sol','fixture','fixture-key',{},fields)
        with self.assertRaises(ValueError):agent_configuration('openai/gpt-6.1-sol','fixture','fixture-key',{}, {},'low')


if __name__=='__main__':unittest.main()

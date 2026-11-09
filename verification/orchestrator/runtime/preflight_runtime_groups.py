#!/usr/bin/env python3
"""Check completed raw groups against the model registry without running TLC.

Emit field names and counts only. Missing registry support or projection
support blocks the next model batch. This is not a conformance check.
"""
import argparse
from collections import Counter,defaultdict
import json
from pathlib import Path
import re
import traceback
from inventory_runtime_groups import collect
from runtime_projection import project
from check_trace import tla
HERE=Path(__file__).resolve().parent


REQUIRED_FIELDS={
 'agent_admission':{'chat','current_attempt','current_run','disposition','effective_access','handle_attempt','handle_run','handle_source_id','handle_source_revision','origin_id','provider_run_id','reason','runtime_id','tool_call_id','validated'},
 'runtime_binding':{'chat','runtime_id','run','attempt','effective_access','origin_id','operation','source_id','source_revision','validated'},
 'capability_catalog':{'scope','complete','providers','policy','policy_sources','core_config','validated'},
 'doc_commit':{'wire_id','effect_id','device','target_host','wire_body_digest'},
 'remote_receive':{'wire_id','wire_body_digest','origin_device','receiver_device','receipt_id','validated'},
 'received_ceiling':{'chat','access','wire_id','effect_id','origin_device','receiver_device','source_kind','receipt_id','epoch','validated'},
 'access_config':{'chat','access','configured_access','submitted_access','config_source','policy_source','validated'},
 'registry_host':{'chat','host','parent','validated'},
 'provider_start':{'chat','run','attempt','effect_id','outcome','effective_access','access_ceiling','journal_seq_before','journal_seq_after'},
 'journal_observed':{'chat','run','attempt','effect_id','journal_seq_before','journal_seq_after'},
 'native_binding':{'chat','task_key','native_path','tool_id','doc_id','chip_doc_id','own_background','effective_background'},
 'transfer_provider':{'adapter','stage','chat','input','session','packet_digest','context_packet_digest','accepted_ids','protocol_assumption'},
 'native_doc_commit':{'chat','task_key','native_path','tool_id','doc_id','chip_doc_id','chip_status','chip_ref','doc_hash','chip_doc_hash'},
 'provider_event':{'chat','provider_run_id','stream_seq','event_id','event_kind','observer'},
 'provider_stream_catalog':{'streams','complete'},
 'engine_lifecycle':{'device','engine_instance_id','source_seq','state'},
 'admission_state':{'device','engine_instance_id','source_seq','state'},
 'transport_connection':{'device','engine_instance_id','source_seq','state','scope','connection_id'},
 'tool_reply':{'parent','parent_run','parent_attempt','child','task_run','reply_id'},
 'tool_observed':{'parent','parent_run','parent_attempt','child','task_run','reply_id'},
 'device_reachability':{'device','online'},
 'transfer_catalog':{'scope','chats','unknown'},
 'transfer_state':{'chat_id','revision','durable','ledger','committed','receipts','runs','rejected','failures','uncertain','providers','queue','entries','version','lineage'},
 'transfer_operation':{'operation','args','before','after','succeeded','journal'},
}


def known_kinds():
    source=(HERE/'RuntimeRefinement.tla').read_text()
    block=source.split('KnownFactKinds ==',1)[1].split('FactSchema',1)[0]
    return set(re.findall(r'"([A-Za-z_][A-Za-z0-9_]*)"',block))


def preflight(root):
    _,index,skipped=collect(root,'engine-integration');known=known_kinds()
    kinds=Counter();shapes=defaultdict(Counter);unknown=Counter();failures=[];actions=Counter();missing_fields=Counter()
    for item in index:
        rows=[json.loads(x) for x in Path(item['path']).read_text().splitlines() if x.strip()]
        seen=set()
        for row in rows:
            actions[row['action']]+=1
            for fact in row['snapshot']['facts']:
                key=(row['dev'],fact['id'])
                if key in seen:continue
                seen.add(key);kind=fact['kind'];kinds[kind]+=1
                if kind not in known:unknown[kind]+=1
                data=fact['data']
                if kind not in REQUIRED_FIELDS:missing_fields[(kind,'<unregistered schema>')]+=1
                elif not isinstance(data,dict):missing_fields[(kind,'<record required>')]+=1
                else:
                    for field in REQUIRED_FIELDS[kind]-set(data):missing_fields[(kind,field)]+=1
                shape=tuple(sorted(data)) if isinstance(data,dict) else ('<non-record>',)
                shapes[kind][shape]+=1
        try:tla(project(rows))
        except Exception as error:
            frame=traceback.extract_tb(error.__traceback__)[-1]
            failures.append(dict(sha256=item['sha256'],error_type=type(error).__name__,module=Path(frame.filename).name,line=frame.lineno))
    required={'provider_stream_catalog','engine_lifecycle','access_config','received_ceiling'}
    if kinds['doc_commit']:required.add('remote_receive')
    missing=sorted(required-set(kinds))
    return dict(check='runtime-schema-preflight',captures=len(index),skipped_manifests=skipped,
       status='pass' if not unknown and not failures and not missing_fields and not missing else 'blocked',
       schema_status='pass' if not unknown and not failures and not missing_fields else 'fail',
       missing_schema_fields=[dict(kind=k,field=f,count=n) for (k,f),n in sorted(missing_fields.items())],
       missing_full_source_fact_kinds=missing,fact_kinds=dict(sorted(kinds.items())),
       unknown_fact_kinds=dict(sorted(unknown.items())),mechanism_actions=dict(sorted(actions.items())),
       fact_shapes={k:[dict(fields=list(shape),count=n) for shape,n in sorted(v.items())] for k,v in sorted(shapes.items())},
       projection_failures=failures,claim='registry/schema/projection only; no conformance result')


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path);p.add_argument('--out',type=Path,required=True)
    a=p.parse_args();result=preflight(a.root);a.out.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k not in {'fact_shapes','projection_failures'}}))
    raise SystemExit(0 if result['status']=='pass' else 2)
if __name__=='__main__':main()

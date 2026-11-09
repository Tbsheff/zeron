"""Keep raw TLC diagnostics with private generated data; print metadata only."""
import json
import re
import subprocess


def run_tlc(out, jar, module, config, metadata=None):
    try:
        result=subprocess.run(['java','-XX:ActiveProcessorCount=4','-cp',str(jar.resolve()),
            'tlc2.TLC','-workers','1','-config',config,module],cwd=out,
            timeout=300,capture_output=True,text=True)
    except subprocess.TimeoutExpired as exc:
        parts=[exc.stdout or '',exc.stderr or '']
        text=''.join(x.decode('utf-8',errors='replace') if isinstance(x,bytes) else x for x in parts)
        (out/'tlc-private.log').write_text(text)
        print(json.dumps({'status':'timeout','seconds':300,**(metadata or {})}),flush=True)
        return 124
    output=result.stdout+result.stderr
    (out/'tlc-private.log').write_text(output)
    success=result.returncode==0 and 'Model checking completed. No error has been found.' in output
    summary={'status':'pass' if success else 'fail',**(metadata or {})}
    cursors=re.findall(r'^cursor = (\d+)$',output,re.M)
    if cursors:summary['row']=int(cursors[-1])
    invariant=re.search(r'Invariant (\w+) is violated',output)
    if invariant:summary['predicate']=invariant.group(1)
    summary['exit_code']=result.returncode
    print(json.dumps(summary,sort_keys=True),flush=True)
    return 0 if success else result.returncode or 1

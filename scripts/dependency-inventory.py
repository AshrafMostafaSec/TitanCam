#!/usr/bin/env python3
"""Export a CycloneDX inventory of the locked Rust graph, not a binary-content assertion."""
import json,sys,datetime
manifest=json.load(open(sys.argv[1],encoding='utf8'))
packages=manifest['packages']
refs={p['id']:f"pkg:cargo/{p['name']}@{p['version']}" for p in packages}
components=[]
for package in packages:
    component={'type':'library','bom-ref':refs[package['id']],'name':package['name'],'version':package['version'],'purl':refs[package['id']]}
    if package.get('license'):component['licenses']=[{'expression':package['license']}]
    else:component['licenses']=[{'license':{'name':'Unspecified; review upstream license file'}}]
    components.append(component)
bom={'bomFormat':'CycloneDX','specVersion':'1.6','version':1,'metadata':{'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00','Z'),'component':{'type':'application','name':'TitanCam','version':next(p['version'] for p in packages if p['name']=='titan-receiver')},'properties':[{'name':'titancam:scope','value':'Locked Rust workspace dependency graph; includes target-dependent packages, not iOS Apple/Opus libraries or system plugins.'}]},'components':components,'dependencies':[{'ref':refs[n['id']],'dependsOn':[refs[d['pkg']] for d in n['deps']]} for n in manifest['resolve']['nodes']]}
json.dump(bom,open(sys.argv[2],'w',encoding='utf8'),indent=2)

from pathlib import Path
with open(sys.argv[3], 'w', encoding='utf8') as notices:
    notices.write('TitanCam locked Rust dependency notices\n\n')
    for package in packages:
        notices.write(f"\n--- {package['name']} {package['version']} ({package.get('license') or 'unspecified'}) ---\n")
        root=Path(package['manifest_path']).parent
        files=sorted({p for pattern in ('LICENSE*','COPYING*','NOTICE*') for p in root.glob(pattern) if p.is_file() and p.stat().st_size < 262144})
        for path in files:
            notices.write(f"\n{path.name}\n")
            notices.write(path.read_text(encoding='utf8',errors='replace')+'\n')

"""Manual Docker smoke test against nuget.org. Administrator key comes only from stdin."""
import json, sys, urllib.request, hashlib
data=json.load(sys.stdin)
base='http://app:5000'
def api(path,body=None):
    headers={'Authorization':'Bearer '+data['admin']}
    if body is not None: headers['Content-Type']='application/json'
    request=urllib.request.Request(base+path,data=json.dumps(body).encode() if body is not None else None,headers=headers)
    with urllib.request.urlopen(request,timeout=120) as response:
        payload=response.read()
        return response.headers,payload
_,rows=api('/api/upstreams')
sources=json.loads(rows)
if not any(source['name']=='nuget.org' for source in sources):
    api('/api/upstreams',{'name':'nuget.org','url':'https://api.nuget.org/v3/index.json','patterns':['*'],'priority':100,'private':False})
path='/v3/flatcontainer/newtonsoft.json/13.0.3/newtonsoft.json.13.0.3.nupkg'
first_headers,first=api(path)
second_headers,second=api(path)
assert first==second and len(first)>1000
assert second_headers['X-NuShelf-Cache']=='HIT'
_,details=api('/api/packages/Newtonsoft.Json')
details=json.loads(details)
assert details['upstream']['name']=='nuget.org'
print(json.dumps({'package':'Newtonsoft.Json','version':'13.0.3','bytes':len(first),'firstCache':first_headers['X-NuShelf-Cache'],'secondCache':second_headers['X-NuShelf-Cache'],'sha512':hashlib.sha512(first).hexdigest(),'metadataOrigin':details['upstream']['name']}))

"""Disposable NuGet origin for real .NET proxy/cache tests. No production use."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import base64, hashlib, json, time, zipfile
from xml.etree import ElementTree

root = Path('/fixtures')
base = 'http://proxy-origin:8080'
downloads = 0

def packages():
    result = {}
    for path in root.glob('*.nupkg'):
        with zipfile.ZipFile(path) as archive:
            xml = ElementTree.fromstring(archive.read(next(n for n in archive.namelist() if n.endswith('.nuspec'))))
            metadata = next(n for n in xml if n.tag.split('}')[-1] == 'metadata')
            fields = {n.tag.split('}')[-1]: n.text or '' for n in metadata}
            result[(fields['id'].lower(), fields['version'].lower())] = (path, fields)
    return result

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        global downloads
        if self.path == '/stats':
            self.send_json({'downloads': downloads}); return
        if (root / '.offline').exists():
            self.send_error(503); return
        if self.path == '/v3/index.json':
            self.send_json({'version':'3.0.0','resources':[
                {'@type':'PackageBaseAddress/3.0.0','@id':base+'/flat/'},
                {'@type':'RegistrationsBaseUrl/3.6.0','@id':base+'/reg/'}]}); return
        parts = self.path.split('/')
        data = packages()
        if len(parts) == 4 and parts[1] == 'flat' and parts[3] == 'index.json':
            versions = sorted(v for package_id,v in data if package_id == parts[2])
            if versions: self.send_json({'versions': versions})
            else: self.send_error(404)
            return
        if len(parts) == 4 and parts[1] == 'reg' and parts[3] == 'index.json':
            entries = [{'catalogEntry':{'id':fields['id'],'version':fields['version'],
                'authors':fields.get('authors','Proxy origin'),'description':fields.get('description','Proxy fixture'),
                'tags':fields.get('tags','proxy'),'listed':True,'published':'2026-01-01T00:00:00Z',
                'dependencyGroups':[]}} for (package_id,_),(_,fields) in data.items() if package_id == parts[2]]
            if entries: self.send_json({'count':1,'items':[{'items':entries,'count':len(entries)}]})
            else: self.send_error(404)
            return
        if len(parts) == 5 and parts[1] == 'flat' and (parts[2],parts[3]) in data:
            payload = data[(parts[2],parts[3])][0].read_bytes()
            if parts[4].endswith('.sha512'):
                payload = base64.b64encode(hashlib.sha512(payload).digest())
            elif parts[4].endswith('.nupkg'):
                downloads += 1
            else: self.send_error(404); return
            self.send_response(200); self.send_header('Content-Length',str(len(payload)))
            self.send_header('Content-Type','application/octet-stream'); self.end_headers(); self.wfile.write(payload); return
        self.send_error(404)

    def send_json(self, value):
        payload = json.dumps(value).encode()
        self.send_response(200); self.send_header('Content-Type','application/json')
        self.send_header('Content-Length',str(len(payload))); self.end_headers(); self.wfile.write(payload)

root.mkdir(parents=True, exist_ok=True)
ThreadingHTTPServer(('0.0.0.0',8080),Handler).serve_forever()

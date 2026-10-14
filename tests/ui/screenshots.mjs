// Capture documentation screenshots from the isolated Docker test application.
// All packages and credentials here are disposable demonstration data.
import { chromium, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { mkdir } from 'node:fs/promises';

const base = process.env.APP_URL || 'http://registry:5000';
const admin = 'test-only-admin-key-at-least-32-characters';
const output = '/results/documentation';
await mkdir(output,{recursive:true});
async function api(path, {method='GET',body,key=admin} = {}) {
  const headers={Authorization:`Bearer ${key}`};
  if (body && !Buffer.isBuffer(body)) { headers['Content-Type']='application/json';body=JSON.stringify(body); }
  const response=await fetch(`${base}${path}`,{method,headers,body});
  if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? null : response.json();
}
const profiles=await api('/api/profiles');
let profile=profiles.find(p => p.name === 'Acme Engineering');
if (!profile) profile=await api('/api/profiles',{method:'POST',body:{name:'Acme Engineering'}});
const createKey = (label,scopes,prefix,days) => api(`/api/profiles/${profile.id}/keys`,{method:'POST',body:{label,scopes,packagePrefix:prefix,expiresAt:new Date(Date.now()+days*86400000).toISOString()}});
const publishing=await createKey('GitHub Actions · Releases',['package:push','package:unlist'],'acme.',90);
await createKey('Local development',['package:push'],'acme.',30);
await createKey('AI package discovery',['mcp:read'],'',30);

const fixtures=[
  {id:'Acme.Core',version:'1.2.0',description:'Shared building blocks for .NET applications. A documentation sample package.',tags:'dotnet core utilities'},
  {id:'Acme.Core',version:'1.3.0',description:'Shared building blocks for .NET applications. A documentation sample package.',tags:'dotnet core utilities'},
  {id:'Acme.Json',version:'2.4.0',description:'JSON serialization helpers for .NET projects. A documentation sample package.',tags:'json serialization dotnet',dependency:true},
  {id:'Acme.Json',version:'2.4.1',description:'JSON serialization helpers for .NET projects. A documentation sample package.',tags:'json serialization dotnet',dependency:true},
  {id:'Acme.Logging',version:'1.8.0',description:'Structured logging conventions for application services. A documentation sample package.',tags:'logging observability aspnet',dependency:true},
  {id:'Acme.Testing',version:'2.9.0',description:'Test utilities for reliable .NET builds. A documentation sample package.',tags:'testing dotnet automation'},
  {id:'Acme.Testing',version:'3.0.0-preview.1',description:'Preview test utilities for .NET builds. A documentation sample package.',tags:'testing dotnet automation'},
];
const zipScript=String.raw`
import sys,json,io,zipfile
from xml.sax.saxutils import escape
p=json.loads(sys.stdin.read())
deps='<dependencies><group targetFramework="net8.0"><dependency id="Acme.Core" version="[1.3.0,)" /></group></dependencies>' if p.get('dependency') else ''
xml=f'<package><metadata><id>{escape(p["id"])}</id><version>{escape(p["version"])}</version><authors>Acme Engineering</authors><description>{escape(p["description"])}</description><tags>{escape(p["tags"])}</tags><license type="expression">MIT</license>{deps}</metadata></package>'
out=io.BytesIO()
with zipfile.ZipFile(out,'w',zipfile.ZIP_DEFLATED) as archive:
    archive.writestr(p['id']+'.nuspec',xml)
    archive.writestr('lib/net8.0/_._',b'')
sys.stdout.buffer.write(out.getvalue())
`;
for (const fixture of fixtures) {
  const body=execFileSync('python3',['-c',zipScript],{input:JSON.stringify(fixture)});
  const response=await fetch(`${base}/api/v2/package`,{method:'PUT',headers:{'X-NuGet-ApiKey':publishing.key},body});
  if (![201,409].includes(response.status)) throw new Error(`Fixture upload: ${response.status} ${await response.text()}`);
}
const browser=await chromium.launch();
try {
  const page=await browser.newPage({viewport:{width:1440,height:1040},deviceScaleFactor:1});
  const errors=[];page.on('pageerror',e => errors.push(e.message));
  await page.goto(`${base}/`);
  await expect(page.locator('.package-row')).toHaveCount(4);
  await page.screenshot({path:`${output}/home.png`,fullPage:true});
  await page.goto(`${base}/packages?q=json`);
  await expect(page.locator('.package-row')).toHaveCount(1);
  await page.screenshot({path:`${output}/search.png`,fullPage:true});
  await page.goto(`${base}/packages/Acme.Json`);
  await expect(page.getByRole('heading',{name:'Version history',exact:true})).toBeVisible();
  await page.screenshot({path:`${output}/package.png`,fullPage:true});
  await page.goto(`${base}/manage`);
  await page.getByLabel('API key',{exact:true}).fill(admin);
  await page.getByRole('button',{name:'Open workspace'}).click();
  await expect(page.locator('#profile-select')).toBeVisible();
  await page.locator('#profile-select').selectOption(profile.id);
  await page.getByRole('link',{name:'API keys & credentials',exact:true}).click();
  await expect(page.getByRole('heading',{name:'Existing keys',exact:true})).toBeVisible();
  await page.screenshot({path:`${output}/management.png`,fullPage:true});
  await page.getByRole('link',{name:'My packages',exact:true}).click();
  await expect(page.getByRole('button',{name:'Unlist',exact:true})).toHaveCount(7);
  await page.screenshot({path:`${output}/owned-packages.png`,fullPage:true});
  await page.setViewportSize({width:390,height:844});
  await page.goto(`${base}/`);
  await expect(page.locator('.package-row')).toHaveCount(4);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({path:`${output}/mobile.png`,fullPage:true});
  expect(errors).toEqual([]);
  console.log('Captured six real application screenshots with demonstration data.');
} finally { await browser.close(); }

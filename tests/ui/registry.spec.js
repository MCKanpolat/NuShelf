import { test, expect } from '@playwright/test';
import { execFileSync } from 'node:child_process';
const admin = 'test-only-admin-key-at-least-32-characters';
test('gallery, management credentials, profile access and responsive layout',async ({page,request}) => {
  const errors=[];page.on('pageerror',e => errors.push(e.message));
  await page.goto('/');
  await expect(page.getByRole('heading',{name:'Your packages. Your registry.'})).toBeVisible();
  await expect(page.locator('[data-stat="packages"]')).not.toHaveText('—');
  await page.screenshot({path:'/results/home-desktop.png',fullPage:true});
  await page.getByRole('link',{name:'Manage workspace'}).click();
  await page.getByLabel('API key',{exact:true}).fill(admin);
  await page.getByRole('button',{name:'Open workspace'}).click();
  await page.getByRole('link',{name:'Publisher profiles',exact:true}).click();
  const name=`UI Team ${Date.now()}`;
  const uiPackageId=`NuShelf.UiSample.${Date.now()}`;
  const uiPackagePath='/tmp/ui-package.nupkg';
  execFileSync('python3',['-c',String.raw`
import sys,zipfile
from xml.etree import ElementTree
with zipfile.ZipFile('/results/ui-sample.nupkg') as source, zipfile.ZipFile('/tmp/ui-package.nupkg','w') as target:
    for entry in source.infolist():
        content=source.read(entry.filename)
        if entry.filename.endswith('.nuspec'):
            xml=ElementTree.fromstring(content)
            for node in xml.iter():
                if node.tag.split('}')[-1]=='id': node.text=sys.argv[1]
            metadata=next(n for n in xml.iter() if n.tag.split('}')[-1]=='metadata')
            ElementTree.SubElement(metadata,'readme').text='README.md'
            content=ElementTree.tostring(xml,encoding='utf-8',xml_declaration=True)
        target.writestr(entry,content)
    target.writestr("README.md","# Embedded documentation\n\n**Published README content**\n\n<script>window.readmeUnsafe=true</script>")
`,uiPackageId]);
  await page.getByLabel('Profile name',{exact:true}).fill(name);
  await page.getByRole('button',{name:'Create profile'}).click();
  await expect(page.getByRole('heading',{name:'API keys & credentials',exact:true})).toBeVisible();
  await page.getByLabel('Key name').fill('Browser test');
  await page.getByRole('button',{name:'Create API key'}).click();
  const dialog=page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  const key=await page.locator('#new-secret').textContent();
  expect(key).toMatch(/^nsh_[a-f0-9]{64}$/);
  await page.getByRole('button',{name:'I’ve saved my key'}).click();
  await expect(page.getByText('Browser test',{exact:true})).toBeVisible();
  // Full secret must disappear from DOM after the one-time dialog closes.
  expect(await page.locator('body').textContent()).not.toContain(key);
  await page.screenshot({path:'/results/management-desktop.png',fullPage:true});
  const stored=await page.evaluate(() => ({local:localStorage.length,session:sessionStorage.length}));
  expect(stored).toEqual({local:0,session:0});
  await page.getByRole('button',{name:'Sign out'}).click();
  await page.getByLabel('API key',{exact:true}).fill(key);
  await page.getByRole('button',{name:'Open workspace'}).click();
  await expect(page.getByRole('heading',{name:`Hello, ${name}.`})).toBeVisible();
  await expect(page.getByRole('link',{name:'Publisher profiles',exact:true})).toHaveCount(0);
  await page.getByLabel('Display name').fill(`${name} renamed`);
  await page.getByRole('button',{name:'Save profile'}).click();
  await expect(page.getByRole('heading',{name:`Hello, ${name} renamed.`})).toBeVisible();
  await page.getByRole('link',{name:'Publish',exact:true}).click();
  await page.getByLabel('Package file',{exact:true}).setInputFiles(uiPackagePath);
  await page.getByRole('button',{name:'Publish package',exact:false}).click();
  await expect(page.locator('#upload-result')).toContainText(`Published ${uiPackageId}`);
  await page.getByRole('link',{name:'View package',exact:false}).click();
  await expect(page.getByRole('heading',{name:'Version history',exact:true})).toBeVisible();
  await expect(page.locator('.readme-content')).toContainText('Published README content');
  expect(await page.evaluate(()=>window.readmeUnsafe)).toBeUndefined();
  await expect(page.getByRole('link',{name:'Download package',exact:false})).toBeVisible();
  await page.screenshot({path:'/results/package-desktop.png',fullPage:true});
  await page.getByRole('link',{name:'Manage workspace'}).click();
  await page.getByRole('link',{name:'My packages',exact:true}).click();
  await page.getByRole('button',{name:'Deprecate',exact:true}).click();
  await page.getByLabel('Message',{exact:true}).fill('Move to the supported package.');
  await page.getByLabel('Alternative package (optional)',{exact:true}).fill('Example.Replacement');
  await page.getByRole('button',{name:'Save deprecation',exact:true}).click();
  await expect(page.getByRole('button',{name:'Clear deprecation',exact:true})).toBeVisible();
  await page.getByRole('link',{name:uiPackageId,exact:true}).click();
  await expect(page.getByText('Deprecated package version',{exact:true})).toBeVisible();
  await expect(page.getByText('Move to the supported package.',{exact:false})).toBeVisible();
  await page.getByRole('link',{name:'Manage workspace'}).click();
  await page.getByRole('link',{name:'My packages',exact:true}).click();
  await page.getByRole('button',{name:'Clear deprecation',exact:true}).click();
  await expect(page.getByRole('button',{name:'Clear deprecation',exact:true})).toHaveCount(0);
  await page.getByRole('button',{name:'Unlist',exact:true}).click();
  await expect(page.getByRole('button',{name:'Relist',exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Relist',exact:true}).click();
  await expect(page.getByRole('button',{name:'Unlist',exact:true})).toBeVisible();
  await page.getByRole('link',{name:'API keys & credentials',exact:true}).click();
  page.once('dialog',dialog => dialog.accept());
  await page.getByRole('button',{name:'Revoke',exact:true}).click();
  await expect(page.getByRole('heading',{name:'Welcome back.'})).toBeVisible();
  const response=await request.get('/api/me',{headers:{Authorization:`Bearer ${key}`}});expect(response.status()).toBe(401);
  await page.goto('/packages?q=not-a-real-package');
  await expect(page.getByRole('heading',{name:'No packages found.'})).toBeVisible();
  await page.getByLabel('Include prerelease').check();
  await expect(page).toHaveURL(/prerelease=true/);
  await page.goto('/docs');
  await expect(page.getByRole('heading',{name:'Connect your AI tools with MCP'})).toBeVisible();
  await page.goto('/');
  await expect(page.locator('.package-row').filter({hasText:uiPackageId})).toHaveCount(1);
  await page.screenshot({path:'/results/home-desktop.png',fullPage:true});
  await page.setViewportSize({width:390,height:844});
  await page.goto('/');
  await expect(page.getByRole('heading',{name:'Your packages. Your registry.'})).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({path:'/results/home-mobile.png',fullPage:true});
  expect(errors).toEqual([]);
});

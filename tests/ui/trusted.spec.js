import {test,expect} from '@playwright/test';
test('trusted publishing policy creation and revocation',async({page,request})=>{
  const admin='test-only-admin-key-at-least-32-characters';const p=await (await request.post('/api/profiles',{headers:{Authorization:`Bearer ${admin}`},data:{name:`CI publisher ${Date.now()}`}})).json();
  await page.goto('/manage');await page.getByLabel('API key',{exact:true}).fill(admin);await page.getByRole('button',{name:'Open workspace'}).click();await page.getByRole('link',{name:'Trusted Publishing',exact:true}).click();
  const name=`CI policy ${Date.now()}`;await page.getByLabel('Policy name',{exact:true}).fill(name);await page.getByLabel('Publisher profile',{exact:true}).last().selectOption(p.id);await page.getByLabel('GitHub repository',{exact:true}).fill('acme/library');await page.getByLabel('Repository ID',{exact:true}).fill('123456');await page.getByLabel('Owner ID',{exact:true}).fill('7890');await page.getByLabel('Package prefix',{exact:true}).fill('Acme.');await page.getByRole('button',{name:'Create publishing policy',exact:true}).click();
  const policy=page.locator('.settings-row').filter({hasText:name});await expect(policy).toBeVisible();await policy.getByRole('button',{name:'Disable & revoke keys',exact:true}).click();await expect(policy).toContainText('Disabled');
});

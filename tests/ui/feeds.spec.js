import {test,expect} from '@playwright/test';
test('private feed creation, member roles and scoped gallery',async({page,request})=>{
  const admin='test-only-admin-key-at-least-32-characters',name=`feed-${Date.now()}`;
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  const p=await (await request.post('/api/profiles',{headers:{Authorization:`Bearer ${admin}`},data:{name:`Feed team ${Date.now()}`}})).json();
  await page.goto('/manage');await page.getByLabel('API key',{exact:true}).fill(admin);await page.getByRole('button',{name:'Open workspace'}).click();
  await page.getByRole('link',{name:'Feeds & teams',exact:true}).click();await page.getByLabel('Feed name',{exact:true}).fill(name);await page.getByRole('button',{name:'Create feed',exact:true}).click();
  const panel=page.locator('.panel').filter({has:page.getByRole('heading',{name:new RegExp(`^${name}`)})});await expect(panel).toContainText('Private');
  await panel.getByLabel('Profile',{exact:true}).selectOption(p.id);await panel.getByLabel('Role',{exact:true}).selectOption('publisher');await panel.getByRole('button',{name:'Save membership',exact:true}).click();
  await expect(page.locator('.panel').filter({hasText:name})).toContainText('publisher');
  await page.screenshot({path:'/results/feeds.png',fullPage:true});
  expect((await request.get(`/feeds/${name}/v3/index.json`)).status()).toBe(401);
  await page.goto(`/feeds/${name}/manage`);await page.getByLabel('API key',{exact:true}).fill(admin);await page.getByRole('button',{name:'Open workspace'}).click();
  await page.getByRole('link',{name:'My packages',exact:true}).click();await expect(page).toHaveURL(new RegExp(`/feeds/${name}/manage`));expect(errors).toEqual([]);
});

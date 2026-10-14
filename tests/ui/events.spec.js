import {test,expect} from '@playwright/test';
test('audit log and webhook management',async({page})=>{
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.goto('/manage');await page.getByLabel('API key',{exact:true}).fill('test-only-admin-key-at-least-32-characters');await page.getByRole('button',{name:'Open workspace'}).click();
  await page.getByRole('link',{name:'Audit log',exact:true}).click();await expect(page.getByRole('heading',{name:'Audit log',exact:true})).toBeVisible();
  await page.getByRole('link',{name:'Webhooks',exact:true}).click();await page.getByLabel('Webhook name',{exact:true}).fill(`UI Hook ${Date.now()}`);await page.getByLabel('Signing secret environment variable',{exact:true}).fill('WEBHOOK_UI_TEST');await page.getByLabel('Receiver URL',{exact:true}).fill('https://example.com/events');await page.getByRole('button',{name:'Create webhook',exact:true}).click();
  await expect(page.locator('#toast')).toHaveText('Webhook created.');await expect(page.getByRole('heading',{name:'Configured receivers',exact:true})).toBeVisible();await page.locator('.settings-row').filter({hasText:'WEBHOOK_UI_TEST'}).last().getByRole('button',{name:'Disable',exact:true}).click();await expect(page.locator('#toast')).toHaveText('Webhook disabled.');expect(errors).toEqual([]);
});

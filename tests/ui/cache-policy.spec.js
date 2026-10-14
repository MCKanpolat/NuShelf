import {test,expect} from '@playwright/test';
test('cache policy saves and dry-run preview is available',async({page})=>{
  await page.goto('/manage');await page.getByLabel('API key',{exact:true}).fill('test-only-admin-key-at-least-32-characters');await page.getByRole('button',{name:'Open workspace'}).click();
  await page.getByRole('link',{name:'Package cache',exact:true}).click();await expect(page.getByRole('heading',{name:'Automatic cache cleanup',exact:true})).toBeVisible();
  await page.getByLabel('Cache quota (MiB)',{exact:true}).fill('1024');await page.getByLabel('Unused for days',{exact:true}).fill('30');await page.getByRole('button',{name:'Save cleanup policy',exact:true}).click();
  await expect(page.getByLabel('Unused for days',{exact:true})).toHaveValue('30');await page.getByRole('button',{name:'Preview cleanup',exact:true}).click();await expect(page.locator('#maintenance-result')).toContainText('cached versions selected');
});

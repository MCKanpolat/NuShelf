import {test,expect} from '@playwright/test';
test('NuGet Audit status and settings',async({page})=>{
  await page.goto('/manage');await page.getByLabel('API key',{exact:true}).fill('test-only-admin-key-at-least-32-characters');await page.getByRole('button',{name:'Open workspace'}).click();
  await page.getByRole('link',{name:'Package security',exact:true}).click();await expect(page.getByRole('heading',{name:'NuGet Audit',exact:true})).toBeVisible();
  await page.getByRole('checkbox',{name:'Synchronize from nuget.org every hour'}).uncheck();await page.getByRole('button',{name:'Save settings',exact:true}).click();await expect(page.locator('#toast')).toHaveText('Audit settings saved.');
  await expect(page.getByRole('checkbox',{name:'Synchronize from nuget.org every hour'})).not.toBeChecked();
});

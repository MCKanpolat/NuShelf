import { test, expect } from '@playwright/test';
const admin = 'test-only-admin-key-at-least-32-characters';

// Playwright hides scrollbars in headless Chromium, which would hide the layout
// shift this spec guards against. Show them like a desktop browser does.
test.use({ launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] } });

test('page content does not shift sideways when a scrollbar appears or disappears', async ({ page }) => {
  // Tall enough that the short pages (the sign-in screen) fit without a scrollbar
  // while long pages (home, documentation) need one.
  await page.setViewportSize({ width: 1280, height: 1400 });
  const lefts = {};
  for (const path of ['/', '/manage', '/packages', '/docs']) {
    await page.goto(path);
    await page.waitForLoadState('networkidle');
    lefts[path] = await page.evaluate(() => Math.round(document.querySelector('.brand').getBoundingClientRect().left));
  }
  expect(new Set(Object.values(lefts)).size, JSON.stringify(lefts)).toBe(1);
});

test('every management screen fits a phone without sideways scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await page.goto('/manage');
  await page.getByLabel('API key', { exact: true }).fill(admin);
  await page.getByRole('button', { name: 'Open workspace' }).click();
  await expect(page.locator('.management-nav')).toBeVisible();
  const tabs = await page.locator('.management-nav a').evaluateAll(links => links.map(a => a.getAttribute('href')));
  expect(tabs.length).toBeGreaterThan(8);
  const overflowing = [];
  for (const href of tabs) {
    await page.locator(`.management-nav a[href="${href}"]`).click();
    await page.waitForLoadState('networkidle');
    const fits = await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
    if (!fits) overflowing.push(href);
  }
  expect(overflowing).toEqual([]);
});

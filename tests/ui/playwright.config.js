import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.js',
  timeout: 30000,
  workers: 1,
  reporter: 'list',
  outputDir: '/results/ui',
  use: {baseURL:process.env.APP_URL || 'http://app:5000',viewport:{width:1440,height:1000},trace:'retain-on-failure'},
});

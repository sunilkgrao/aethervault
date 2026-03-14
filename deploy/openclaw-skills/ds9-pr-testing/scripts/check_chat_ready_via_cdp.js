#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');

let chromium;
try {
  ({ chromium } = require('playwright'));
} catch (error) {
  console.error('playwright-not-installed');
  console.error(String(error && error.message ? error.message : error));
  process.exit(2);
}

async function main() {
  const [, , endpoint, baseUrl = 'http://localhost:5173', screenshotPath = ''] =
    process.argv;

  if (!endpoint) {
    console.error('usage: check_chat_ready_via_cdp.js ENDPOINT [BASE_URL] [SCREENSHOT_PATH]');
    process.exit(2);
  }

  const browser = await chromium.connectOverCDP(endpoint);
  const context = browser.contexts()[0] || (await browser.newContext());
  let page =
    context
      .pages()
      .find(
        (candidate) =>
          typeof candidate.url === 'function' &&
          candidate.url().startsWith(baseUrl),
      ) || context.pages()[0];

  if (!page) {
    page = await context.newPage();
  }

  if (!page.url().startsWith(baseUrl)) {
    await page.goto(baseUrl, { waitUntil: 'domcontentloaded' });
  } else {
    await page.bringToFront();
    await page.reload({ waitUntil: 'domcontentloaded' });
  }

  const chatButton = page.locator('button[aria-label="chat"]').first();
  await chatButton.waitFor({ state: 'visible', timeout: 15000 }).catch(() => {});
  if ((await chatButton.count()) === 0 || !(await chatButton.isVisible().catch(() => false))) {
    throw new Error('chat-button-not-found');
  }

  const chatHeading = page.getByRole('heading', { name: 'Chat with Tribble' }).first();
  if (!(await chatHeading.isVisible().catch(() => false))) {
    await chatButton.click();
  }

  await chatHeading.waitFor({ state: 'visible', timeout: 10000 });

  const textarea = page.locator('textarea:visible').first();

  await textarea.waitFor({ state: 'visible', timeout: 10000 });

  const result = await textarea.evaluate((node) => ({
    placeholder: node.getAttribute('placeholder') || '',
    disabled: Boolean(node.disabled),
    ariaDisabled: node.getAttribute('aria-disabled') || '',
    readOnly: Boolean(node.readOnly),
    value: node.value || '',
  }));

  const final = {
    endpoint,
    baseUrl,
    pageUrl: page.url(),
    title: await page.title(),
    ...result,
  };

  if (screenshotPath) {
    const resolved = path.resolve(screenshotPath);
    fs.mkdirSync(path.dirname(resolved), { recursive: true });
    await page.screenshot({ path: resolved, fullPage: true });
    final.screenshot = resolved;
  }

  console.log(JSON.stringify(final, null, 2));

  const pageOrigin = new URL(final.pageUrl).origin;
  const baseOrigin = new URL(baseUrl).origin;
  if (pageOrigin !== baseOrigin) {
    console.error('unexpected-page-url');
    process.exit(3);
  }

  if (final.disabled || final.readOnly || final.placeholder !== 'Type your message') {
    console.error('chat-not-ready');
    process.exit(4);
  }

  process.exit(0);

}

main().catch((error) => {
  console.error(String(error && error.stack ? error.stack : error));
  process.exit(1);
});

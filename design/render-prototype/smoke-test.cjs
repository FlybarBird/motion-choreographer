const assert = require('node:assert/strict');
const path = require('node:path');
const { chromium } = require('playwright');

(async () => {
  const browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH || '/usr/bin/chromium',
    headless: true,
    args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader'],
  });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('response', (response) => { if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`); });
  try {
    await page.goto(process.env.PROTOTYPE_URL || 'http://127.0.0.1:8765');
    await page.waitForFunction(() => window.renderStudy?.stats.frames > 2);
    const settle = async () => {
      const frames = await page.evaluate(() => renderStudy.stats.frames);
      await page.waitForFunction((frames) => renderStudy.stats.frames > frames + 2, frames);
    };
    for (const view of ['home', 'lectern', 'overview', 'rig']) {
      await page.selectOption('#view', view);
      await page.waitForFunction(() => document.getElementById('cpu-image').complete && document.getElementById('cpu-image').naturalWidth === 960);
      let simple;
      for (const geometry of ['simple', 'detailed']) {
        await page.locator(`input[name=geometry][value=${geometry}]`).check();
        for (const lighting of ['simple', 'studio']) {
          await page.locator(`input[name=lighting][value=${lighting}]`).check();
          await settle();
          const current = await page.evaluate(() => ({ ...renderStudy.state, triangles: renderStudy.triangles, drawn: renderStudy.stats.submittedTriangles }));
          assert.equal(current.view, view);
          assert.equal(current.geometry, geometry);
          assert.equal(current.lighting, lighting);
          assert(current.drawn > 0, 'The renderer must submit actual triangles');
          if (geometry === 'simple') simple = current.triangles;
          else assert(current.triangles > simple, 'Detailed geometry should replace the block fixture');
        }
      }
      if (view === 'home') await page.screenshot({ path: path.join(__dirname, 'comparison.png'), fullPage: true });
      if (view === 'rig') await page.screenshot({ path: path.join(__dirname, 'comparison-rig.png'), fullPage: true });
    }
    const canvas = await page.locator('#candidate-canvas').boundingBox();
    await page.mouse.move(canvas.x + canvas.width / 2, canvas.y + canvas.height / 2);
    await page.mouse.down();
    await page.mouse.move(canvas.x + canvas.width / 2 + 50, canvas.y + canvas.height / 2 + 25, { steps: 4 });
    await page.mouse.up();
    assert.equal(await page.evaluate(() => renderStudy.state.custom), true);
    await page.getByRole('button', { name: 'Reset view' }).click();
    assert.equal(await page.evaluate(() => renderStudy.state.custom), false);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.selectOption('#view', 'home');
    await settle();
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'Mobile layout must not overflow horizontally');
    await page.screenshot({ path: path.join(__dirname, 'comparison-mobile.png'), fullPage: true });
    assert.deepEqual(errors, []);
    console.log('Passed: 4 views × 4 geometry/lighting combinations, CPU image loads, orbit/reset, mobile layout, and no browser errors.');
  } finally {
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });

import { expect, test } from "@playwright/test";
import { installGuiMocks } from "./fixtures/mock-control-client";

for (const viewport of [
  { width: 1120, height: 760 },
  { width: 1280, height: 800 },
  { width: 1440, height: 900 },
]) {
  test(`overview fits desktop viewport ${viewport.width}x${viewport.height}`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await installGuiMocks(page);
    await page.goto("/");
    await expect(page.getByRole("navigation", { name: /desktop sections/i })).toBeVisible();
    await expect(page.getByText("TOTAL COST")).toBeVisible();
    await expect(page.getByText("Estimated Saved")).toHaveCount(0);
    await expect(page.getByText("Routed Share")).toHaveCount(0);
    const chartBox = await page.locator(".overview-console__chart").boundingBox();
    const figureBox = await page.getByRole("figure", { name: /usage over time/i }).boundingBox();
    expect(chartBox).not.toBeNull();
    expect(figureBox).not.toBeNull();
    expect(chartBox!.height).toBeGreaterThan(300);
    expect(figureBox!.height).toBeGreaterThan(280);
  });
}

test("no horizontal overflow at minimum width", async ({ page }) => {
  await page.setViewportSize({ width: 1120, height: 760 });
  await installGuiMocks(page);
  await page.goto("/");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
  expect(overflow).toBe(false);
});

test("activity scroll stays below the sticky desktop titlebar at minimum width", async ({ page }) => {
  await page.setViewportSize({ width: 1120, height: 760 });
  await installGuiMocks(page);
  await page.goto("/");

  await page.getByRole("button", { name: /activity/i }).click();

  const content = page.locator(".desktop-workspace__content");
  await expect(content).toBeVisible();
  await content.evaluate((element) => {
    element.scrollTo({ top: element.scrollHeight });
  });

  const titlebarBox = await page.locator(".desktop-titlebar").boundingBox();
  const tableBox = await page.locator(".activity-page__table-shell").boundingBox();
  expect(titlebarBox).not.toBeNull();
  expect(tableBox).not.toBeNull();

  const hitTarget = await page.evaluate(({ x, y }) => {
    return document.elementFromPoint(x, y)?.closest(".desktop-titlebar, .activity-page__table-shell")?.className ?? null;
  }, {
    x: Math.round(titlebarBox!.x + titlebarBox!.width / 2),
    y: Math.round(titlebarBox!.y + Math.min(titlebarBox!.height - 8, 24)),
  });

  expect(hitTarget).toContain("desktop-titlebar");
  await expect(page.getByRole("heading", { name: "Activity" })).toBeVisible();
});


test("activity uses 100-row pages by default and loads the next page via cursor pagination", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installGuiMocks(page);
  await page.goto("/");

  await page.getByRole("button", { name: /activity/i }).click();

  const table = page.getByRole("table", { name: /activity ledger/i });
  await expect(table).toBeVisible();
  await expect(table.locator("tbody tr")).toHaveCount(100);
  await expect(table.getByRole("cell", { name: "Claude Code 1", exact: true })).toBeVisible();
  await expect(page.getByText("Showing 210 items")).toBeVisible();

  await page.getByRole("button", { name: "Next page" }).click();

  await expect(table.locator("tbody tr")).toHaveCount(100);
  await expect(table.getByRole("cell", { name: "Claude Code 101", exact: true })).toBeVisible();
  await expect(page.getByText("Showing 210 items")).toBeVisible();
});

test("prompt palette page is reachable from sidebar", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installGuiMocks(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Prompt Palette" }).click();
  await expect(page.getByRole("heading", { name: "Prompt Palette" })).toBeVisible();
  await expect(page.getByText("Review Diff")).toBeVisible();
});

test("prompt palette overlay opens with keyboard shortcut", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installGuiMocks(page);
  await page.goto("/");
  await page.keyboard.press(process.platform === "darwin" ? "Meta+Shift+K" : "Control+Shift+K");
  await expect(page.getByRole("dialog", { name: /prompt palette/i })).toBeVisible();
  await expect(page.getByRole("searchbox", { name: /search prompts/i })).toBeFocused();
});

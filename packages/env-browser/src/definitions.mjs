import { z } from "zod";

export const definitions = {
  browser_navigate: { description: "Navigate the session's browser page to an HTTP or HTTPS URL.",
    input: z.strictObject({ url: z.url().refine(value => ["http:", "https:"].includes(new URL(value).protocol), "use HTTP or HTTPS") }) },
  browser_inspect: { description: "Read the current page URL, title and accessible page structure.", input: z.strictObject({}) },
  browser_click: { description: "Click one element using a Playwright locator selector.", input: z.strictObject({ selector: z.string().min(1) }) },
  browser_fill: { description: "Fill one form field using a Playwright locator selector.", input: z.strictObject({ selector: z.string().min(1), value: z.string() }) },
  browser_screenshot: { description: "Capture the current viewport as an image visible to the model.", input: z.strictObject({}) },
};

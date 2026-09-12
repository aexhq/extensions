import { publish } from "../../../shared/media.mjs";
import { z } from "zod";
import { definitions } from "./definitions.mjs";
import { toolOutput } from "../../../shared/tool-output.mjs";
import { environmentHandler, bindingKey, accepted, result, unknown, failure, fail, EnvironmentError } from "../../../shared/environment-server.mjs";
export { serveEnvironment } from "../../../shared/environment-server.mjs";

const configuration = z.strictObject({ profile: z.string().min(1) });
const descriptor = z.strictObject({ type: z.literal("aex_browser_tool"), version: z.literal(1), name: z.enum(Object.keys(definitions)) });

export function createBrowserEnvironment({ profiles, publishMedia }) {
  const launchers = new Map(Object.entries(profiles));
  for (const launch of launchers.values()) if (typeof launch !== "function") throw new TypeError("each browser profile must supply a launcher");
  const bindings = new Map();
  async function pageFor(state) {
    if (state.lost) fail("resource_lost", "the browser was closed or interrupted; create a new binding explicitly");
    if (!state.browser) {
      state.browser = await launchers.get(state.profile)();
      state.browser.on("disconnected", () => { state.lost = true; });
      try {
        state.context = await state.browser.newContext({ viewport: { width: 1280, height: 720 }, serviceWorkers: "block", acceptDownloads: false });
        state.page = await state.context.newPage();
        state.page.on("close", () => { state.lost = true; });
        state.page.on("crash", () => { state.lost = true; });
        state.context.on("page", page => { if (page !== state.page) void page.close().catch(() => {}); });
      } catch (error) { state.lost = true; await state.browser.close(); throw error; }
    }
    if (state.lost || !state.browser.isConnected()) fail("resource_lost", "the browser is unavailable");
    return state.page;
  }
  async function execute(op, state) {
    const parsed = descriptor.safeParse(op.request.implementation);
    if (!parsed.success) fail("unsupported", "unsupported browser implementation descriptor");
    const name = parsed.data.name;
    const input = definitions[name].input.parse(op.request.input);
    if (state.active.has(op.sequence)) fail("busy", "this invocation is already active");
    const controller = new AbortController();
    const call = { controller, started: false, closing: undefined };
    state.active.set(op.sequence, call);
    const timer = setTimeout(() => controller.abort(new EnvironmentError("timeout", "browser execution deadline expired")), op.request.deadline_ms);
    const abort = () => {
      if (call.started) {
        state.lost = true;
        if (state.browser) call.closing = state.browser.close();
        call.closing?.catch(() => {});
      }
    };
    controller.signal.addEventListener("abort", abort, { once: true });
    const work = state.tail.then(async () => {
      controller.signal.throwIfAborted();
      if (state.lost) fail("resource_lost", "the browser was lost; create a new binding explicitly");
      call.started = true;
      const page = await pageFor(state);
      if (controller.signal.aborted) { abort(); controller.signal.throwIfAborted(); }
      page.setDefaultTimeout(op.request.deadline_ms);
      if (name === "browser_navigate") await page.goto(input.url, { waitUntil: "domcontentloaded" });
      if (name === "browser_click") await page.locator(input.selector).click();
      if (name === "browser_fill") await page.locator(input.selector).fill(input.value);
      if (name === "browser_screenshot") {
        const png = await page.screenshot({ type: "png", timeout: op.request.deadline_ms });
        return result(toolOutput({ url: page.url(), title: await page.title() }, [await publish(publishMedia, png, "image/png", 0, { sessionId: op.session_id, sequence: op.sequence, signal: controller.signal })]));
      }
      const output = { url: page.url(), title: await page.title() };
      if (name === "browser_inspect") {
        const snapshot = await page.locator("body").ariaSnapshot({ timeout: op.request.deadline_ms });
        output.snapshot = snapshot.slice(0, 64 * 1024);
        output.truncated = snapshot.length > 64 * 1024;
      }
      return result(output);
    });
    state.tail = work.catch(() => {});
    let receipt;
    try {
      receipt = await work;
      controller.signal.throwIfAborted();
    } catch (error) {
      if (error.name === "TimeoutError" && call.started) {
        controller.abort(new EnvironmentError("timeout", error.message));
      }
      if (controller.signal.aborted) receipt = failure(controller.signal.reason.code, controller.signal.reason.message);
      else if (call.started && state.lost) receipt = unknown(error.message);
      else throw error;
    } finally {
      clearTimeout(timer);
      controller.signal.removeEventListener("abort", abort);
      try { await call.closing; }
      catch (error) {
        if (receipt.type === "failure") receipt.details = { ...receipt.details, cleanup_error: error.message };
        else if (receipt.type === "unknown") receipt = unknown(`${receipt.message}; browser cleanup failed: ${error.message}`);
        else receipt = failure("cleanup_failed", `browser cleanup failed: ${error.message}`, { output: receipt.output });
      }
      finally { state.active.delete(op.sequence); }
    }
    return receipt;
  }
  const handle = environmentHandler(async op => {
    const key = bindingKey(op);
    const request = op.request;
    if (request.type === "setup") {
      const { profile } = configuration.parse(request.configuration);
      if (!launchers.has(profile)) fail("unsupported", "unknown browser profile");
      if (bindings.has(key)) fail("already_attached", "browser binding already exists");
      bindings.set(key, { profile, lost: false, deleted: false, tail: Promise.resolve(), active: new Map() });
      return accepted();
    }
    const state = bindings.get(key);
    if (!state) {
      fail("resource_lost", "this controller has no browser for the binding; create a new binding explicitly");
    }
    if (request.type === "teardown" && state.deleted) return accepted();
    if (state.deleted) fail("unavailable", "browser binding was deleted");
    if (request.type === "execute") return execute(op, state);
    if (request.type === "cancel") {
      const call = state.active.get(request.target_sequence);
      if (call) { call.controller.abort(new EnvironmentError("cancelled", "browser execution cancelled")); await call.closing; }
      return accepted();
    }
    if (["detach", "teardown"].includes(request.type)) {
      if (state.active.size) fail("busy", "browser has active invocations");
      if (request.type === "teardown") {
        await state.browser?.close();
        state.deleted = true;
      }
      return accepted();
    }
    fail("unsupported", "browser lifecycle operation is not supported");
  });
  return { handle, close: async () => { await Promise.all([...bindings.values()].map(state => state.browser?.close())); } };
}

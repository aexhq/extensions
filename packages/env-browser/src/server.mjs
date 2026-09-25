import { publish } from "../../../shared/media.mjs";
import { z } from "zod";
import { definitions } from "./definitions.mjs";
import { methods } from "./methods.mjs";
import { toolOutput } from "../../../shared/tool-output.mjs";
import { reportEnvironment, finishExecution, deadlineTimer, environmentHandler, bindingKey, accepted, result, unknown, failure, fail, EnvironmentError } from "../../../shared/environment-server.mjs";
export { serveEnvironment } from "../../../shared/environment-server.mjs";

const configuration = z.strictObject({ profile: z.string().min(1) });
const descriptor = z.strictObject({ type: z.literal("aex_browser_tool"), version: z.literal(1), name: z.enum(Object.keys(definitions)) });

export function createBrowserEnvironment({ profiles, publishMedia, maxPages = 16, fetch = globalThis.fetch }) {
  z.number().int().positive().parse(maxPages);
  const launchers = new Map(Object.entries(profiles));
  for (const launch of launchers.values()) if (typeof launch !== "function") throw new TypeError("each browser profile must supply a launcher");
  const bindings = new Map();
  function observe(state, observation) {
    state.reporting = reportEnvironment(state.reporter, observation, fetch).catch(error => {
      state.reportingError = error.message;
      console.error("Browser Environment observation was not acknowledged");
    });
  }
  async function openPage(state, name) {
    if (state.pages.has(name)) fail("already_exists", "a page with this name already exists");
    if (state.pages.size >= maxPages) fail("capacity", "browser page limit reached");
    state.creatingPage = true;
    let page;
    try { page = await state.context.newPage(); }
    finally { state.creatingPage = false; }
    state.pages.set(name, page);
    state.selected = name;
    for (const event of ["close", "crash"]) page.on(event, () => {
      state.pages.delete(name);
      if (state.selected === name) state.selected = undefined;
      if (!state.lost && !state.deleted) observe(state, { scope: "resource", resource: name, code: `page_${event}`, message: `Browser page ${name} ${event}` });
    });
    return page;
  }
  async function pageFor(state) {
    if (state.lost) fail("resource_lost", "the browser was closed or interrupted; create a new binding explicitly");
    if (!state.browser) {
      state.browser = await launchers.get(state.profile)();
      state.browser.on("disconnected", () => {
        state.lost = true;
        if (!state.deleted) observe(state, { scope: "environment", availability: "unavailable", message: "The browser disconnected; inspect it and create a new Environment explicitly if needed." });
      });
      try {
        state.context = await state.browser.newContext({ viewport: { width: 1280, height: 720 }, serviceWorkers: "block", acceptDownloads: false });
        state.context.on("page", page => { if (!state.creatingPage) void page.close().catch(() => {}); });
        await openPage(state, "main");
      } catch (error) { state.lost = true; await state.browser.close(); throw error; }
    }
    if (state.lost || !state.browser.isConnected()) fail("resource_lost", "the browser is unavailable");
    const page = state.pages.get(state.selected);
    if (!page) fail("page_unavailable", "open or select a page before using browser Tools");
    return page;
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
    const cancelTimer = deadlineTimer(op.request.deadline_ms, () => controller.abort(new EnvironmentError("timeout", "browser execution deadline expired")));
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
      page.setDefaultTimeout(0);
      if (name === "browser_navigate") await page.goto(input.url, { waitUntil: "domcontentloaded" });
      if (name === "browser_click") await page.locator(input.selector).click();
      if (name === "browser_fill") await page.locator(input.selector).fill(input.value);
      if (name === "browser_screenshot") {
        const png = await page.screenshot({ type: "png", timeout: 0 });
        return result(toolOutput({ url: page.url(), title: await page.title() }, [await publish(publishMedia, png, "image/png", 0, { sessionId: op.session_id, sequence: op.sequence, signal: controller.signal })]));
      }
      const output = { url: page.url(), title: await page.title() };
      if (name === "browser_inspect") {
        const snapshot = await page.locator("body").ariaSnapshot({ timeout: 0 });
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
      cancelTimer();
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
      bindings.set(key, { profile, reporter: op.reporter, pages: new Map(), lost: false, deleted: false, tail: Promise.resolve(), active: new Map() });
      return accepted();
    }
    const state = bindings.get(key);
    if (!state) {
      fail("resource_lost", "this controller has no browser for the binding; create a new binding explicitly");
    }
    if (request.type === "teardown" && state.deleted) return accepted();
    if (state.deleted) fail("unavailable", "browser binding was deleted");
    if (request.type === "call") {
      const method = Object.hasOwn(methods, request.name) && methods[request.name];
      if (!method) fail("unsupported", "unknown browser method");
      const input = method.input.parse(request.input);
      if (request.name === "inspect") return result({ profile: state.profile, lost: state.lost,
        started: state.browser !== undefined, selected: state.selected ?? null,
        pages: [...state.pages].map(([name, page]) => ({ name, url: page.url(), closed: page.isClosed() })),
        activeInvocations: [...state.active.keys()], reportingError: state.reportingError ?? null });
      const work = state.tail.then(async () => {
        if (!state.browser) await pageFor(state);
        if (state.lost) fail("resource_lost", "the browser is unavailable");
        if (request.name === "open_page") await openPage(state, input.name);
        else {
          const page = state.pages.get(input.name);
          if (!page) fail("not_found", "unknown browser page");
          if (request.name === "select_page") state.selected = input.name;
          if (request.name === "close_page") await page.close();
        }
        return result({ selected: state.selected ?? null, pages: [...state.pages.keys()] });
      });
      state.tail = work.catch(() => {});
      return work;
    }
    if (request.type === "execute") return finishExecution(op, await execute(op, state), fetch);
    if (request.type === "cancel") {
      const call = state.active.get(request.target_sequence);
      if (call) { call.controller.abort(new EnvironmentError("cancelled", "browser execution cancelled")); await call.closing; }
      return accepted();
    }
    if (["detach", "teardown"].includes(request.type)) {
      if (state.active.size) fail("busy", "browser has active invocations");
      if (request.type === "teardown") {
        state.reporter = undefined;
        await state.browser?.close();
        await state.reporting;
        state.deleted = true;
      }
      return accepted();
    }
    fail("unsupported", "browser lifecycle operation is not supported");
  });
  return { handle, close: async () => { await Promise.all([...bindings.values()].map(state => state.browser?.close())); } };
}

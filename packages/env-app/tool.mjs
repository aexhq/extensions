import { typed } from "@aexhq/model";
import { invoke as invokeEnvironment } from "aex:tool/environment@1.0.0";

import { dispatch } from "./dispatcher.mjs";

export function invoke(request) {
  return typed("invoke", () => dispatch(invokeEnvironment, request), "app_tool");
}

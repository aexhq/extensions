export interface SseEvent { event?: string; data: string }
export declare class SseDecoder {
  constructor(maxFrame?: number);
  readonly pending: number;
  feed(chunk: Uint8Array): SseEvent[];
}
export declare function parseJson(value: string, field: string): unknown;
export declare function usage(fields: Record<string, number | undefined>): Record<string, number>;
export declare function terminal(stopReason: string | undefined): string;

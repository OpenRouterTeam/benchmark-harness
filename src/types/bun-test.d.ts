declare module "bun" {
  interface OnLoadArgs {
    path: string;
    namespace: string;
  }
  interface OnLoadResult {
    contents: string;
    loader: "js" | "jsx" | "ts" | "tsx" | "json" | "toml" | "css" | "text";
  }
  interface PluginBuilder {
    onLoad: (
      constraints: {
        filter: RegExp;
        namespace?: string;
      },
      callback: (args: OnLoadArgs) => OnLoadResult | Promise<OnLoadResult>
    ) => void;
  }
  interface BunPlugin {
    name: string;
    setup: (build: PluginBuilder) => void;
  }
  export function plugin(plugin: BunPlugin): void;
  export function resolveSync(specifier: string, parent: string): string;
}
declare module "bun:test" {
  type TestFn = () => void | Promise<void>;
  interface It {
    (label: string, fn: TestFn, timeout?: number): void;
    (
      label: string,
      options: {
        timeout?: number;
      },
      fn: TestFn
    ): void;
    each: {
      <T extends readonly unknown[]>(
        cases: readonly T[]
      ): (
        label: string,
        fn: (...args: [...T]) => void | Promise<void>,
        timeout?: number
      ) => void;
      <T>(
        cases: readonly T[]
      ): (
        label: string,
        fn: (arg: T) => void | Promise<void>,
        timeout?: number
      ) => void;
    };
    serial: It;
    skip: (label: string, fn: TestFn, timeout?: number) => void;
    only: (label: string, fn: TestFn, timeout?: number) => void;
    todo: (label: string) => void;
  }
  interface Describe {
    (label: string, fn: () => void): void;
    (
      label: string,
      options: {
        timeout?: number;
      },
      fn: () => void
    ): void;
    skip: Describe;
    only: Describe;
    concurrent: Describe;
    todo: (label: string) => void;
    skipIf: (condition: boolean) => Describe;
    if: (condition: boolean) => Describe;
    each: {
      <T extends readonly unknown[]>(
        cases: readonly T[]
      ): {
        (
          label: string,
          options: {
            timeout?: number;
          },
          fn: (...args: [...T]) => void | Promise<void>
        ): void;
        (
          label: string,
          fn: (...args: [...T]) => void | Promise<void>,
          timeout?: number
        ): void;
      };
      <T>(cases: readonly T[]): {
        (
          label: string,
          options: {
            timeout?: number;
          },
          fn: (arg: T) => void | Promise<void>
        ): void;
        (
          label: string,
          fn: (arg: T) => void | Promise<void>,
          timeout?: number
        ): void;
      };
    };
  }
  interface Expect {
    (actual: any, message?: string): any;
    stringContaining: (expected: string) => unknown;
    stringMatching: (expected: string | RegExp) => unknown;
    objectContaining: (expected: Record<string, unknown>) => unknown;
    arrayContaining: (expected: readonly unknown[]) => unknown;
    any: (ctor: new (...args: any[]) => any) => any;
    anything: () => unknown;
    assertions: (count: number) => void;
    hasAssertions: () => void;
    not: Expect;
    extend: (
      matchers: Record<
        string,
        (...args: any[]) => {
          pass: boolean;
          message: () => string;
        }
      >
    ) => void;
  }
  export const expect: Expect;
  export const describe: Describe;
  export const it: It;
  export const test: It;
  interface Mock<F extends (...args: any[]) => any = (...args: any[]) => any> {
    (...args: Parameters<F>): ReturnType<F>;
    mock: {
      calls: any[][];
      results: {
        type: "return" | "throw";
        value: any;
      }[];
      lastCall: any[] | undefined;
    };
    mockImplementation: (fn: (...args: any[]) => any) => Mock<F>;
    mockImplementationOnce: (fn: (...args: any[]) => any) => Mock<F>;
    mockReturnValue: (value: ReturnType<F>) => Mock<F>;
    mockReturnValueOnce: (value: ReturnType<F>) => Mock<F>;
    mockResolvedValue: (value: any) => Mock<F>;
    mockResolvedValueOnce: (value: any) => Mock<F>;
    mockRejectedValue: (value: any) => Mock<F>;
    mockRejectedValueOnce: (value: any) => Mock<F>;
    mockReturnThis: () => Mock<F>;
    mockClear: () => Mock<F>;
    mockReset: () => Mock<F>;
    mockRestore: () => void;
  }
  interface MockFn {
    <F extends (...args: any[]) => any>(fn?: F): Mock<F>;
    module: (path: string, factory: () => any) => void;
  }
  export const mock: MockFn;
  export function spyOn<T extends object, K extends keyof T>(
    obj: T,
    method: K,
    accessor?: "get" | "set"
  ): Mock;
  interface Jest {
    clearAllMocks: () => void;
    restoreAllMocks: () => void;
    useFakeTimers: (options?: any) => void;
    useRealTimers: () => void;
    setSystemTime: (time?: number | Date) => void;
    advanceTimersByTime: (ms: number) => Promise<void>;
    runOnlyPendingTimers: () => Promise<void>;
  }
  export const jest: Jest;
  export function setSystemTime(now?: number | Date): void;
  export const beforeAll: (fn: TestFn, timeout?: number) => void;
  export const afterAll: (fn: TestFn, timeout?: number) => void;
  export const beforeEach: (fn: TestFn, timeout?: number) => void;
  export const afterEach: (fn: TestFn, timeout?: number) => void;
}

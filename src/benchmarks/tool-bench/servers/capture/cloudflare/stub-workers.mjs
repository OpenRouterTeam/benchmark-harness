// Stand-in for the `cloudflare:workers` module outside workerd. Bindings are only
// touched when tools are called, never while listing them.
const missing = (path) =>
  new Proxy(function () {}, {
    get: (_t, key) => (key === 'then' ? undefined : missing(`${path}.${String(key)}`)),
    apply: () => {
      throw new Error(`binding ${path} is not available during capture`)
    }
  })
export const env = missing('env')
export class WorkerEntrypoint {}
export class DurableObject {}
export const exports = missing('exports')
export class WorkflowEntrypoint {}
export class RpcTarget {}
export const waitUntil = () => {}

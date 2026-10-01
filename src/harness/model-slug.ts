const ROUTING_PREFIX = "openrouter/";

export function stripRoutingPrefix(model: string): string {
  if (!model.startsWith(ROUTING_PREFIX)) {
    return model;
  }
  const rest = model.slice(ROUTING_PREFIX.length);
  return rest.includes("/") ? rest : model;
}

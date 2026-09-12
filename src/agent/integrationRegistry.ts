export interface ToolDefinition {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
  };
}

export interface IntegrationToolModule<Name extends string = string> {
  definitions: readonly ToolDefinition[];
  protectedTools?: readonly Name[];
  requiresConfirmation?(name: Name, args: Record<string, unknown>): boolean;
  execute(name: Name, args: Record<string, unknown>): Promise<unknown>;
  summary?(name: Name, args: Record<string, unknown>): string;
}

const modules = new Map<string, IntegrationToolModule>();

export function registerIntegrationTools(module: IntegrationToolModule): void {
  for (const definition of module.definitions) {
    if (modules.has(definition.function.name)) throw new Error(`Duplicate integration tool: ${definition.function.name}`);
    modules.set(definition.function.name, module);
  }
}

export function integrationDefinitions(): ToolDefinition[] {
  return [...new Set(modules.values())].flatMap((module) => [...module.definitions]);
}

export function integrationProtectedTools(): string[] {
  return [...new Set(modules.values())].flatMap((module) => [...(module.protectedTools ?? [])]);
}

export function integrationRequiresConfirmation(name: string, args: Record<string, unknown>): boolean {
  const module = modules.get(name);
  if (!module) return false;
  return module.requiresConfirmation?.(name, args) ?? module.protectedTools?.includes(name) ?? false;
}

export function hasIntegrationTool(name: string): boolean { return modules.has(name); }

export async function executeIntegrationTool(name: string, args: Record<string, unknown>): Promise<unknown> {
  const module = modules.get(name);
  if (!module) throw new Error(`Unknown integration tool: ${name}`);
  return module.execute(name, args);
}

export function integrationToolSummary(name: string, args: Record<string, unknown>): string | undefined {
  return modules.get(name)?.summary?.(name, args);
}

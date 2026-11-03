// Runs cloudflare/mcp's real createServer() (Code Mode, the default) over stdio so
// toolmeter can capture its tools/list. Spec artifacts come from the repo's generated files.
import { readFileSync } from 'node:fs'
import { createServer } from './src/server'
import { setSpecArtifactSource } from './src/isolate-cache'
import { StdioServerTransport } from '@modelcontextprotocol/server/stdio'

setSpecArtifactSource(async (name) => readFileSync(`src/generated/spec/${name}.txt`, 'utf8'))

const server = await createServer({
  type: 'user_token',
  accessToken: 'cf-toolbench',
  user: { id: 'toolbench-user', email: 'ops@octo-corp.example' },
  accounts: [{ id: '0123456789abcdef0123456789abcdef', name: 'Octo Corp' }]
} as never)
await server.connect(new StdioServerTransport())

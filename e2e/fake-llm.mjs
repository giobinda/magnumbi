// A stand-in for an AI provider (OpenAI format), for the website's screenshots only: it asks
// Magnum BI's tools for what a real model would, and answers from what they return.
// Usage: node e2e/fake-llm.mjs <port> <recipient email>
import http from 'node:http'

const [port, recipient] = [Number(process.argv[2] ?? 8097), process.argv[3] ?? 'someone@example.com']
const call = (name, args) => ({ choices: [{ message: { role: 'assistant', content: null,
  tool_calls: [{ id: `call_${Date.now()}`, type: 'function', function: { name, arguments: JSON.stringify(args) } }] } }] })
const say = text => ({ choices: [{ message: { role: 'assistant', content: text } }] })
const money = n => `$${Math.round(n / 1000)}k`

function answer(messages) {
  const last = messages[messages.length - 1]
  const question = [...messages].reverse().find(m => m.role === 'user')?.content ?? ''
  if (/monday/i.test(question)) {
    if (last.role === 'user') {
      return call('create_report', { dashboard: 'sales_overview', report: { name: 'Monday revenue', format: 'csv',
        recipients: [recipient], when: { every: 'weekly', day: 1, at: '08:00', timezone: 'Europe/Rome' } } })
    }
    return say('I\'ve prepared a weekly report of the Sales overview, every Monday at 08:00 (Rome time), by email to you. Confirm it below and it starts next Monday.')
  }
  if (last.role === 'user') {
    return call('run_query', { query: { model: 'sales', dimensions: ['sold_at:month'], measures: ['revenue'],
      period: { field: 'sold_at', current: { last_n_days: 92 } } } })
  }
  const result = JSON.parse(last.content)
  const rows = (result.rows ?? []).filter(r => typeof r[1] === 'number' || !isNaN(Number(r[1])))
  // whole months only: the window starts and ends part-way through one
  if (rows.length < 4) return say('There isn\'t enough data in the last three months to compare.')
  const [a, b] = [rows[1], rows[rows.length - 2]]
  const change = Math.round(((Number(b[1]) - Number(a[1])) / Number(a[1])) * 100)
  return say(`Revenue went from ${money(Number(a[1]))} in ${String(a[0]).slice(0, 7)} to ${money(Number(b[1]))} in ${String(b[0]).slice(0, 7)} (${change >= 0 ? '+' : ''}${change}%); the current month is still in progress. Based on the certified Store sales model.`)
}

http.createServer((req, res) => {
  let body = ''
  req.on('data', c => { body += c })
  req.on('end', () => {
    const reply = answer(JSON.parse(body || '{}').messages ?? [])
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify(reply))
  })
}).listen(port, '127.0.0.1')

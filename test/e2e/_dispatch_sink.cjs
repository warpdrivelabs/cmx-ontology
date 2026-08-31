// 本地投递汇聚 sink：捕获 onto dispatcher 的**真出站请求**（webhook + startBusinessProcess）。
// 每条请求以一行 JSON {method,path,headers,body} 追加到 CAP 文件；
//   POST /api/flow/v1/instances → 返 flow 信封 {code:0,data:{id}}（模拟 flowengine 起实例）
//   POST /hook/*                → 返 200 {ok:true}
//   GET  /__ping                → 就绪探针
// 仅用 Node 内建 http/fs，无三方依赖。用法：CAP=/tmp/x.jsonl SINK_PORT=8770 node _dispatch_sink.cjs
'use strict';
const http = require('http');
const fs = require('fs');
const PORT = Number(process.env.SINK_PORT || 8770);
const CAP = process.env.CAP || '/tmp/onto-sink-captured.jsonl';
try { fs.writeFileSync(CAP, ''); } catch (e) { /* */ }
let seq = 0;

const srv = http.createServer((req, res) => {
  const path = req.url.split('?')[0];
  if (req.method === 'GET' && path === '/__ping') { res.writeHead(200); res.end('ok'); return; }
  const chunks = [];
  req.on('data', c => chunks.push(c));
  req.on('end', () => {
    const raw = chunks.length ? Buffer.concat(chunks).toString('utf8') : '';
    let body; try { body = JSON.parse(raw); } catch { body = raw; }
    try { fs.appendFileSync(CAP, JSON.stringify({ method: req.method, path, headers: req.headers, body }) + '\n'); } catch (e) { /* */ }
    res.writeHead(200, { 'Content-Type': 'application/json' });
    if (path === '/api/flow/v1/instances') {
      seq += 1;
      res.end(JSON.stringify({ code: 0, msg: 'success', data: { id: 'mock-inst-' + seq, activeNodes: ['approve'] } }));
    } else {
      res.end(JSON.stringify({ ok: true }));
    }
  });
});
srv.listen(PORT, '127.0.0.1', () => console.log('sink listening on 127.0.0.1:' + PORT + ' cap=' + CAP));

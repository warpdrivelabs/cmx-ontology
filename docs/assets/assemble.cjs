// 装配：读模板，把 {{IMG:name}} 替换为 assets/svg/name.svg 的 base64 data-URI，产出自包含 md。
const fs = require('fs'), path = require('path');
const SVG = '/Users/nanomesh/Workspace/presentation/cmx-ontology/docs/assets/svg';
const TPL = process.argv[2];
const OUT = process.argv[3];
let md = fs.readFileSync(TPL, 'utf8');
let n = 0;
md = md.replace(/\{\{IMG:([a-z0-9_]+)\}\}/g, (_, name) => {
  const p = path.join(SVG, name + '.svg');
  const b64 = Buffer.from(fs.readFileSync(p)).toString('base64');
  n++;
  return 'data:image/svg+xml;base64,' + b64;
});
fs.writeFileSync(OUT, md);
console.log(`wrote ${OUT} · ${md.length} bytes · ${n} images inlined`);

// Ayudante CDP mínimo: node cdp.js eval "<expresión JS>"   |   node cdp.js shot salida.png
const fs = require("fs");
async function connect() {
  const list = await (await fetch("http://127.0.0.1:9333/json")).json();
  const page = list.find((t) => t.type === "page");
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); }
  };
  const send = (method, params = {}) => new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
  return { ws, send };
}
(async () => {
  const [cmd, arg] = process.argv.slice(2);
  const { ws, send } = await connect();
  if (cmd === "eval") {
    const r = await send("Runtime.evaluate", { expression: arg, awaitPromise: true, returnByValue: true });
    console.log(JSON.stringify(r.result.exceptionDetails ? { error: r.result.exceptionDetails.exception?.description || r.result.exceptionDetails.text } : r.result.result.value, null, 1));
  } else if (cmd === "shot") {
    const r = await send("Page.captureScreenshot", { format: "png" });
    fs.writeFileSync(arg, Buffer.from(r.result.data, "base64"));
    console.log("guardado", arg);
  }
  ws.close();
})().catch((e) => { console.error("ERROR", e.message); process.exit(1); });

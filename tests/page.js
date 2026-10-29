// Runs the page's own script (argv[2] is ui.html) against a small stand-in DOM and a stub
// fetch, then prints what each scenario left on the page as JSON.
const html = require("fs").readFileSync(process.argv[2], "utf8");
const script = html.slice(html.indexOf("<script>") + 8, html.lastIndexOf("</script>"));

class El {
  constructor() { Object.assign(this, { attrs: {}, children: [], dataset: {}, on: {}, value: "", textContent: "" }); }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return this.attrs[k] ?? null; }
  removeAttribute(k) { delete this.attrs[k]; }
  append(...c) { this.children.push(...c); }
  replaceChildren(...c) { this.children = c; }
  addEventListener(type, f) { (this.on[type] ??= []).push(f); }
  fire(type) { for (const f of this.on[type] ?? []) f({ preventDefault() {}, target: this }); }
  requestSubmit() { this.fire("submit"); }
  focus() { document.activeElement = this; }
  get text() { return [this.textContent, ...this.children.map(c => (typeof c === "string" ? c : c.text))].join(" "); }
}
El.prototype.classList = { add() {}, remove() {} };

const found = {};
const document = Object.assign(new El(), {
  body: new El(),
  createElement: () => new El(),
  querySelector: s => (found[s] ??= new El()),
});
document.activeElement = document.body;
const sent = [];
const sleep = ms => new Promise(r => setTimeout(r, ms));
// The first upload answers slowly, as a large file or a slow read does.
const fetch = async (url, init) => {
  const name = decodeURIComponent(init.headers["X-File-Name"]);
  sent.push(name);
  if (name === "slow.png") await sleep(400);
  return { json: async () => ({ path: name, type: "png", pages: [], barcodes: [], warnings: [], error: null }) };
};
new Function("document", "fetch", "navigator", script)(document, fetch, { userAgent: "" });

const $ = s => document.querySelector(s);
const choose = name => { $("#files").files = [{ name, size: 1 }]; $("#files").fire("change"); };

(async () => {
  choose("slow.png");
  await sleep(25);
  choose("fast.png");
  await sleep(600);
  const busy = $("#out").getAttribute("aria-busy");

  $("#dpi").value = "71";
  sent.length = 0;
  choose("low-dpi.png");
  await sleep(50);
  console.log(JSON.stringify({
    busy,
    dpi: { sent, open: $("details").open === true, shown: $("#out").text.replace(/\s+/g, " ").trim(), focused: document.activeElement === $("#dpi") },
  }));
})();

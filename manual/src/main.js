import { Marked } from "marked";
import ptz from "../../docs/ptz-pro.md?raw";
import setup from "../../docs/setup-guide.md?raw";
import programming from "../../docs/programming-manual.md?raw";

const BOOKS = [
  { id: "ptz-pro", short: "PTZ Pro", blurb: "Setup and programming from the panel", source: ptz },
  { id: "setup", short: "Setup", blurb: "From a new machine to a working show", source: setup },
  { id: "programming", short: "Programming", blurb: "Build, rehearse and play a show in the app", source: programming },
];

const FILE_TO_BOOK = {
  "ptz-pro.md": "ptz-pro",
  "setup-guide.md": "setup",
  "programming-manual.md": "programming",
};

function githubSlug(text) {
  return text
    .toLowerCase()
    .trim()
    .replace(/[\u2000-\u206F\u2E00-\u2E7F\\'!"#$%&()*+,./:;<=>?@[\]^`{|}~]/g, "")
    .replace(/\s/g, "-");
}

function rewriteHref(href, bookId) {
  if (!href) return href;
  if (href.startsWith("#")) return `#/${bookId}/${href.slice(1)}`;
  for (const [file, id] of Object.entries(FILE_TO_BOOK)) {
    if (href === file) return `#/${id}`;
    if (href.startsWith(`${file}#`)) return `#/${id}/${href.slice(file.length + 1)}`;
  }
  return href;
}

function compile(book) {
  const seen = new Map();
  const marked = new Marked();
  marked.use({
    gfm: true,
    renderer: {
      heading({ tokens, depth }) {
        const inner = this.parser.parseInline(tokens);
        const plain = inner.replace(/<[^>]+>/g, "");
        let slug = githubSlug(plain);
        const n = seen.get(slug) ?? 0;
        seen.set(slug, n + 1);
        if (n > 0) slug = `${slug}-${n}`;
        return `<h${depth} id="${slug}">${inner}</h${depth}>\n`;
      },
      link({ href, title, tokens }) {
        const inner = this.parser.parseInline(tokens);
        const fixed = rewriteHref(href, book.id);
        const titleAttr = title ? ` title="${escapeAttr(title)}"` : "";
        const external = /^https?:/i.test(fixed);
        const extra = external ? ' target="_blank" rel="noreferrer"' : "";
        return `<a href="${escapeAttr(fixed)}"${titleAttr}${extra}>${inner}</a>`;
      },
    },
  });
  let html = marked.parse(book.source);
  html = html.replaceAll("<table>", '<div class="table-wrap"><table>').replaceAll("</table>", "</table></div>");
  const doc = new DOMParser().parseFromString(`<div>${html}</div>`, "text/html");
  const root = doc.body.firstElementChild;
  const outline = [...root.querySelectorAll("h2, h3")].map((h) => ({
    id: h.id,
    depth: h.tagName === "H2" ? 2 : 3,
    text: h.textContent.trim(),
  }));
  const sections = outline.map((item, i) => {
    const start = root.querySelector(`#${CSS.escape(item.id)}`);
    const stopId = outline.slice(i + 1).find((next) => next.depth <= item.depth)?.id;
    const stop = stopId ? root.querySelector(`#${CSS.escape(stopId)}`) : null;
    let text = "";
    for (let node = start; node && node !== stop; node = node.nextElementSibling) {
      text += ` ${node.textContent}`;
    }
    return { ...item, text: text.replace(/\s+/g, " ").trim() };
  });
  return { html: root.innerHTML, outline, sections };
}

function escapeAttr(value) {
  return value.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;");
}

const compiled = new Map(BOOKS.map((book) => [book.id, compile(book)]));

const app = document.querySelector("#app");
app.innerHTML = `
  <header class="top">
    <button class="contents" type="button" aria-expanded="false">Contents</button>
    <a class="mark" href="#/ptz-pro">PTZ Programmer</a>
    <nav class="books" aria-label="Manuals">
      ${BOOKS.map((book) => `<a data-book="${book.id}" href="#/${book.id}">${book.short}</a>`).join("")}
    </nav>
    <form class="find" role="search">
      <input type="search" placeholder="Search the manuals" aria-label="Search the manuals" autocomplete="off" />
      <div class="hits" hidden></div>
    </form>
  </header>
  <div class="body">
    <aside class="side" aria-label="On this page"></aside>
    <main class="sheet"></main>
  </div>
`;

const contentsButton = app.querySelector(".contents");
const side = app.querySelector(".side");
const sheet = app.querySelector(".sheet");
const search = app.querySelector('input[type="search"]');
const hits = app.querySelector(".hits");
const bookLinks = [...app.querySelectorAll(".books a")];

function parseHash() {
  const raw = location.hash.replace(/^#\/?/, "");
  const [bookId, anchor] = raw.split("/");
  const book = BOOKS.find((item) => item.id === bookId) ?? BOOKS[0];
  return { book, anchor: anchor || "" };
}

function render() {
  const { book, anchor } = parseHash();
  const page = compiled.get(book.id);
  document.title = `${book.short} · PTZ Programmer`;
  for (const link of bookLinks) {
    link.classList.toggle("on", link.dataset.book === book.id);
  }
  side.innerHTML = `
    <nav class="side-books" aria-label="Manuals">
      ${BOOKS.map((item) => `<a class="${item.id === book.id ? "on" : ""}" href="#/${item.id}">${item.short}</a>`).join("")}
    </nav>
    <p class="side-title">${book.short}</p>
    <p class="side-blurb">${book.blurb}</p>
    <nav>
      ${page.outline
        .map(
          (item) =>
            `<a class="d${item.depth}" href="#/${book.id}/${item.id}">${item.text}</a>`,
        )
        .join("")}
    </nav>
  `;
  sheet.innerHTML = page.html;
  sheet.scrollTop = 0;
  if (anchor) {
    const target = sheet.querySelector(`#${CSS.escape(anchor)}`);
    if (target) target.scrollIntoView();
  }
  markCurrent();
  closeContents();
}

function markCurrent() {
  const links = [...side.querySelectorAll("nav:not(.side-books) a")];
  const line = sheet.getBoundingClientRect().top + 96;
  let current = links[0];
  for (const link of links) {
    const id = link.getAttribute("href").split("/").pop();
    const heading = sheet.querySelector(`#${CSS.escape(id)}`);
    if (heading && heading.getBoundingClientRect().top <= line) current = link;
  }
  for (const link of links) link.classList.toggle("here", link === current);
}

function closeContents() {
  document.body.classList.remove("nav-open");
  contentsButton.setAttribute("aria-expanded", "false");
}

contentsButton.addEventListener("click", () => {
  const open = document.body.classList.toggle("nav-open");
  contentsButton.setAttribute("aria-expanded", String(open));
});

side.addEventListener("click", (event) => {
  if (event.target.closest("a")) closeContents();
});

sheet.addEventListener("click", (event) => {
  const link = event.target.closest("a");
  if (!link) return;
  const href = link.getAttribute("href") || "";
  if (!href.startsWith("#/")) return;
  event.preventDefault();
  if (location.hash === href) {
    const anchor = href.split("/")[2];
    const target = anchor && sheet.querySelector(`#${CSS.escape(anchor)}`);
    if (target) target.scrollIntoView();
    return;
  }
  location.hash = href;
});

sheet.addEventListener("scroll", markCurrent, { passive: true });
window.addEventListener("hashchange", render);

function searchNow() {
  const query = search.value.trim().toLowerCase();
  if (query.length < 2) {
    hits.hidden = true;
    hits.innerHTML = "";
    return;
  }
  const found = [];
  for (const book of BOOKS) {
    for (const section of compiled.get(book.id).sections) {
      const at = section.text.toLowerCase().indexOf(query);
      if (at < 0) continue;
      const from = Math.max(0, at - 40);
      const snippet = section.text.slice(from, at + query.length + 70).trim();
      found.push({ book, section, snippet });
      if (found.length === 12) break;
    }
    if (found.length === 12) break;
  }
  hits.hidden = false;
  if (found.length === 0) {
    hits.innerHTML = `<p class="none">Nothing in the manuals matches.</p>`;
    return;
  }
  hits.innerHTML = found
    .map(
      (hit) => `
        <a href="#/${hit.book.id}/${hit.section.id}">
          <span>${hit.book.short}</span>
          <strong>${hit.section.text}</strong>
          <em>${hit.snippet}</em>
        </a>`,
    )
    .join("");
}

search.addEventListener("input", searchNow);
search.form.addEventListener("submit", (event) => event.preventDefault());
hits.addEventListener("click", () => {
  hits.hidden = true;
  search.value = "";
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    hits.hidden = true;
    closeContents();
  }
  if (event.key === "/" && document.activeElement !== search) {
    event.preventDefault();
    search.focus();
  }
});
document.addEventListener("click", (event) => {
  if (!event.target.closest(".find")) hits.hidden = true;
});

if (!location.hash.startsWith("#/")) location.hash = "#/ptz-pro";
render();

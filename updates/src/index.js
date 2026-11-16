// @ts-check
// The update service. Two questions are answered, both only to a caller that sends the update key:
//
//   GET /v1/latest/<platform>           the newest release's signed description, for macos-arm64 and so on
//   GET /v1/download/<version>/<file>   a short-lived address the installer can be downloaded from
//
// Nothing here is trusted by the app: the description is signed on the machine that made the release
// and names the installer's size and SHA-256, and the app checks all three. This service only reaches
// the private repository on the app's behalf.

const GITHUB = "https://api.github.com";
/** A release's description is a few hundred bytes. Anything large under that name is not one. */
const DESCRIPTION_MOST = 16 * 1024;

/**
 * @typedef {{ GITHUB_REPO: string, GITHUB_TOKEN: string, UPDATE_KEY: string }} Env
 * @typedef {{ name: string, url: string, size: number }} Asset
 */

export default {
  /**
   * @param {Request} request
   * @param {Env} env
   */
  async fetch(request, env) {
    const url = new URL(request.url);
    try {
      const response = await answer(request, env, url);
      if (response.status >= 400) {
        log("refused", { path: url.pathname, status: response.status });
      }
      return response;
    } catch (e) {
      console.error(JSON.stringify({ message: "failed", path: url.pathname, error: e instanceof Error ? e.message : String(e) }));
      return problem(502, "The update service could not reach the releases. Try again in a minute");
    }
  },
};

/**
 * @param {Request} request
 * @param {Env} env
 * @param {URL} url
 */
async function answer(request, env, url) {
  if (request.method !== "GET") {
    return problem(405, "Only GET is answered");
  }
  if (url.pathname === "/") {
    return new Response("PTZ Programmer update service\n", { headers: { "content-type": "text/plain; charset=utf-8" } });
  }
  if (!env.GITHUB_TOKEN || !env.UPDATE_KEY) {
    return problem(503, "The update service is not set up yet: its secrets are missing");
  }
  if (!(await same(request.headers.get("x-ptz-key") ?? "", env.UPDATE_KEY))) {
    return problem(401, "This copy of the app is not known to the update service");
  }
  const latest = url.pathname.match(/^\/v1\/latest\/([a-z0-9]+-[a-z0-9_]+)$/);
  if (latest) {
    return describe(env, latest[1]);
  }
  const download = url.pathname.match(/^\/v1\/download\/(\d+\.\d+\.\d+)\/([A-Za-z0-9][A-Za-z0-9._-]*)$/);
  if (download) {
    return installer(env, download[1], download[2]);
  }
  return problem(404, "There is nothing at this address");
}

/**
 * The newest release's description and its signature, exactly as they were published.
 * @param {Env} env
 * @param {string} platform
 */
async function describe(env, platform) {
  const release = await github(env, `/repos/${env.GITHUB_REPO}/releases/latest`);
  if (release.status === 404) {
    return problem(404, "No release has been published yet");
  }
  if (!release.ok) {
    return upstream(release);
  }
  /** @type {{ tag_name: string, assets: Asset[] }} */
  const found = await release.json();
  const name = `update-${platform}.json`;
  const manifest = found.assets.find((a) => a.name === name);
  const signature = found.assets.find((a) => a.name === `${name}.sig`);
  if (!manifest || !signature) {
    return problem(404, `Release ${found.tag_name} has nothing for ${platform}`);
  }
  if (manifest.size > DESCRIPTION_MOST || signature.size > DESCRIPTION_MOST) {
    return problem(502, `Release ${found.tag_name} is not described the way this service expects`);
  }
  const [text, sig] = await Promise.all([contents(env, manifest), contents(env, signature)]);
  log("latest", { platform, release: found.tag_name });
  return json(200, { manifest: text, signature: sig.trim() });
}

/**
 * Where one release's installer can be downloaded from, for the next few minutes.
 * @param {Env} env
 * @param {string} version
 * @param {string} file
 */
async function installer(env, version, file) {
  // Only an installer of that release, by the names scripts/package.sh gives them.
  if (!new RegExp(`^(PTZ|ptz)-(Programmer|Server|programmer|server)-${version.replaceAll(".", "\\.")}-[a-z0-9_-]+\\.(pkg|run)$`).test(file)) {
    return problem(404, "That is not the name of an installer");
  }
  const release = await github(env, `/repos/${env.GITHUB_REPO}/releases/tags/v${version}`);
  if (release.status === 404) {
    return problem(404, `There is no release ${version}`);
  }
  if (!release.ok) {
    return upstream(release);
  }
  /** @type {{ assets: Asset[] }} */
  const found = await release.json();
  const asset = found.assets.find((a) => a.name === file);
  if (!asset) {
    return problem(404, `Release ${version} has no ${file}`);
  }
  const at = await signed(env, asset);
  log("download", { version, file, size: asset.size });
  return new Response(null, { status: 302, headers: { location: at, "cache-control": "no-store" } });
}

/**
 * @param {Env} env
 * @param {string} path
 */
function github(env, path) {
  return fetch(GITHUB + path, { headers: headers(env, "application/vnd.github+json") });
}

/**
 * @param {Env} env
 * @param {string} accept
 */
function headers(env, accept) {
  return {
    authorization: `Bearer ${env.GITHUB_TOKEN}`,
    accept,
    "user-agent": "ptz-updates",
    "x-github-api-version": "2022-11-28",
  };
}

/**
 * The address GitHub hands out for a file of a private release: good for a few minutes, and for
 * anyone who has it, so the token is not sent on to it.
 * @param {Env} env
 * @param {Asset} asset
 */
async function signed(env, asset) {
  const response = await fetch(asset.url, { headers: headers(env, "application/octet-stream"), redirect: "manual" });
  const at = response.headers.get("location");
  if (response.status !== 302 || !at) {
    await response.body?.cancel();
    throw new Error(`GitHub answered ${response.status} for ${asset.name} where a download address was expected`);
  }
  return at;
}

/**
 * A small file of a release, as text.
 * @param {Env} env
 * @param {Asset} asset
 */
async function contents(env, asset) {
  const response = await fetch(await signed(env, asset));
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error(`GitHub answered ${response.status} for the contents of ${asset.name}`);
  }
  return response.text();
}

/**
 * GitHub would not answer: say which of the two usual reasons it is.
 * @param {Response} response
 */
async function upstream(response) {
  await response.body?.cancel();
  if (response.status === 401) {
    return problem(502, "GitHub refused the update service's token. It has expired or been removed");
  }
  if (response.status === 403 || response.status === 429) {
    return problem(502, "GitHub is not answering the update service just now. Try again in a few minutes");
  }
  return problem(502, `GitHub answered ${response.status} to the update service`);
}

/**
 * Compared in constant time, whatever their lengths.
 * @param {string} given
 * @param {string} expected
 */
async function same(given, expected) {
  const bytes = new TextEncoder();
  const [a, b] = await Promise.all([
    crypto.subtle.digest("SHA-256", bytes.encode(given)),
    crypto.subtle.digest("SHA-256", bytes.encode(expected)),
  ]);
  // @ts-ignore timingSafeEqual is the Workers runtime's addition to Web Crypto.
  return crypto.subtle.timingSafeEqual(a, b);
}

/**
 * @param {number} status
 * @param {unknown} body
 */
function json(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
}

/**
 * @param {number} status
 * @param {string} error
 */
function problem(status, error) {
  return json(status, { error });
}

/**
 * @param {string} message
 * @param {Record<string, unknown>} fields
 */
function log(message, fields) {
  console.log(JSON.stringify({ message, ...fields }));
}

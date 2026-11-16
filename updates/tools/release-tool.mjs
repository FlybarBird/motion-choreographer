// What scripts/release.sh needs that a shell cannot do: the keys a release is made with, and the
// signed description of a release that the app checks before it installs anything.
//
//   node release-tool.mjs keys                                   make the keys if they are missing
//   node release-tool.mjs describe <version> <platform> <installer> <notes file> <out folder>
//
// The keys are kept in ~/.ptz-release (or $PTZ_RELEASE_HOME), never in the repository:
//   signing-key.pem   signs each release. Whoever has it can make an update every app will install.
//   update-key        what the app sends to the update service. Built into released apps.
// The public half of the signing key is crates/app/update-signing.pub, which is in the repository.
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, randomBytes, sign, verify } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const home = process.env.PTZ_RELEASE_HOME || join(homedir(), ".ptz-release");
const signingKey = join(home, "signing-key.pem");
const updateKey = join(home, "update-key");
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const publicKey = join(root, "crates", "app", "update-signing.pub");

function fail(why) {
  console.error(`error: ${why}`);
  process.exit(1);
}

/** The 32 bytes of an Ed25519 public key, as hex. */
function publicHex(key) {
  return Buffer.from(createPublicKey(key).export({ format: "jwk" }).x, "base64url").toString("hex");
}

function keys() {
  mkdirSync(home, { recursive: true, mode: 0o700 });
  chmodSync(home, 0o700);
  if (!existsSync(signingKey)) {
    const { privateKey } = generateKeyPairSync("ed25519");
    writeFileSync(signingKey, privateKey.export({ format: "pem", type: "pkcs8" }), { mode: 0o600 });
    console.log(`Made the signing key: ${signingKey}`);
  }
  if (!existsSync(updateKey)) {
    writeFileSync(updateKey, randomBytes(24).toString("hex") + "\n", { mode: 0o600 });
    console.log(`Made the update key: ${updateKey}`);
  }
  const hex = publicHex(createPrivateKey(readFileSync(signingKey)));
  const known = existsSync(publicKey) ? readFileSync(publicKey, "utf8").trim() : "";
  if (known !== hex) {
    if (/^[0-9a-f]{64}$/.test(known) && !/^0+$/.test(known)) {
      fail(`${publicKey} is the public half of another signing key. Apps already installed trust that one: put its signing key back in ${home}`);
    }
    writeFileSync(publicKey, hex + "\n");
    console.log(`Wrote its public half to ${publicKey}`);
  }
}

function describe(version, platform, installer, notesFile, out) {
  if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) fail("the version must be three numbers, like 0.1.2");
  if (!existsSync(signingKey)) fail(`there is no signing key in ${home}. Run: node ${basename(process.argv[1])} keys`);
  const key = createPrivateKey(readFileSync(signingKey));
  if (publicHex(key) !== readFileSync(publicKey, "utf8").trim()) {
    fail(`the signing key in ${home} is not the one apps trust (${publicKey})`);
  }
  const bytes = readFileSync(installer);
  const manifest = JSON.stringify(
    {
      version,
      platform,
      installer: { name: basename(installer), size: statSync(installer).size, sha256: createHash("sha256").update(bytes).digest("hex") },
      notes: readFileSync(notesFile, "utf8").trim(),
      published: new Date().toISOString(),
    },
    null,
    2,
  );
  const signature = sign(null, Buffer.from(manifest), key);
  if (!verify(null, Buffer.from(manifest), createPublicKey(key), signature)) fail("the signature does not check");
  const name = join(out, `update-${platform}.json`);
  writeFileSync(name, manifest);
  writeFileSync(`${name}.sig`, signature.toString("hex") + "\n");
  console.log(name);
}

const [command, ...rest] = process.argv.slice(2);
if (command === "keys") keys();
else if (command === "describe" && rest.length === 5) describe(...rest);
else fail("use: release-tool.mjs keys | describe <version> <platform> <installer> <notes file> <out folder>");

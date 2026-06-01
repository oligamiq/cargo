import { WASI, File, OpenFile, ConsoleStdout } from "npm:@bjorn3/browser_wasi_shim";

const wasmFile = Deno.args[0];
if (!wasmFile) {
  console.error("Usage: deno run -A test_run.ts <path_to_cargo.wasm> [args...]");
  Deno.exit(1);
}

const wasmBytes = await Deno.readFile(wasmFile);

const wasiArgs = ["cargo", ...Deno.args.slice(1)];
const envObj = Deno.env.toObject();
if (!envObj["HOME"] && envObj["USERPROFILE"]) {
    envObj["HOME"] = envObj["USERPROFILE"];
}
if (!envObj["HOME"]) {
    envObj["HOME"] = "/";
}
const wasiEnv = Object.entries(envObj).map(([k, v]) => `${k}=${v}`);
const fds = [
    new OpenFile(new File([])), // stdin
    ConsoleStdout.lineBuffered((msg) => console.log(msg)), // stdout
    ConsoleStdout.lineBuffered((msg) => console.error(msg)), // stderr
];
const wasi = new WASI(wasiArgs, wasiEnv, fds);

let wasmInstance: WebAssembly.Instance;
let wasiExtAllocate: (size: number) => number;
let memory: WebAssembly.Memory;

function getMemory(): Uint8Array {
  return new Uint8Array(memory.buffer);
}

function readString(ptr: number, len: number): string {
  const mem = getMemory();
  return new TextDecoder().decode(mem.subarray(ptr, ptr + len));
}

const envImports = {
  wasi_ext_fetch: (
    methodPtr: number, methodLen: number,
    urlPtr: number, urlLen: number,
    headersPtr: number, headersLen: number,
    bodyPtr: number, bodyLen: number,
    outStatus: number,
    outRespPtr: number,
    outRespLen: number,
  ): number => {
    try {
      const method = readString(methodPtr, methodLen);
      const url = readString(urlPtr, urlLen);
      const headersStr = readString(headersPtr, headersLen);
      let body: Uint8Array | null = null;
      if (bodyLen > 0) {
        body = getMemory().subarray(bodyPtr, bodyPtr + bodyLen).slice();
      }

      console.log(`[Host] Fetching: ${method} ${url}`);

      const args = ["-s", "-i", "-X", method, url];
      for (const line of headersStr.split('\n')) {
        if (line.trim()) {
          args.push("-H", line.trim());
        }
      }

      const cmd = new Deno.Command("curl", { args, stdout: "piped", stderr: "piped" });
      const output = cmd.outputSync();
      
      if (!output.success) {
        console.error("[Host] curl failed", new TextDecoder().decode(output.stderr));
        return 1;
      }

      const respStr = new TextDecoder().decode(output.stdout).replace(/\r\n/g, '\n');
      const lines = respStr.split('\n');
      let status = 200;
      if (lines[0].startsWith("HTTP/")) {
        const parts = lines[0].split(' ');
        if (parts.length >= 2) {
          status = parseInt(parts[1], 10);
        }
      }

      const outBuf = new TextEncoder().encode(respStr);
      const ptr = wasiExtAllocate(outBuf.length);
      getMemory().set(outBuf, ptr);

      const view = new DataView(memory.buffer);
      view.setUint16(outStatus, status, true);
      view.setUint32(outRespPtr, ptr, true);
      view.setUint32(outRespLen, outBuf.length, true);

      return 0; // Success
    } catch (e) {
      console.error("[Host] Fetch Error:", e);
      return 1;
    }
  },
  wasi_ext_git_clone: (urlPtr: number, urlLen: number, destPtr: number, destLen: number): number => {
    const url = readString(urlPtr, urlLen);
    const dest = readString(destPtr, destLen);
    console.log(`[Host] Git Clone: ${url} -> ${dest}`);
    const cmd = new Deno.Command("git", { args: ["clone", url, dest] });
    return cmd.outputSync().success ? 0 : 1;
  },
  wasi_ext_git_fetch: (pathPtr: number, pathLen: number): number => {
    const path = readString(pathPtr, pathLen);
    console.log(`[Host] Git Fetch in: ${path}`);
    const cmd = new Deno.Command("git", { args: ["-C", path, "fetch"] });
    return cmd.outputSync().success ? 0 : 1;
  }
};

const module = await WebAssembly.compile(wasmBytes);
wasmInstance = await WebAssembly.instantiate(module, {
  "wasi_snapshot_preview1": wasi.wasiImport,
  "env": envImports,
});

memory = wasmInstance.exports.memory as WebAssembly.Memory;
wasiExtAllocate = wasmInstance.exports.wasi_ext_allocate as (size: number) => number;

wasi.start(wasmInstance);

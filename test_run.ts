// test_run.ts - Final Robust WASI Implementation for Deno supporting Cargo Build

const wasmFile = Deno.args[0];
if (!wasmFile) {
  console.error("Usage: deno run -A test_run.ts <path_to_cargo.wasm> [args...]");
  Deno.exit(1);
}

const wasmBytes = await Deno.readFile(wasmFile);

const wasiArgs = ["cargo", ...Deno.args.slice(1)];
const envObj = Deno.env.toObject();

const hostCwd = Deno.cwd().replace(/\\/g, "/");
const hostHome = (Deno.env.get("HOME") || Deno.env.get("USERPROFILE") || hostCwd).replace(/\\/g, "/");

// Set critical environment variables to prevent Cargo from searching for its own exe
envObj["HOME"] = "/home";
envObj["CARGO_HOME"] = "/home/.cargo";
envObj["CARGO"] = "/cargo.wasm";
envObj["RUST_BACKTRACE"] = "full";
if (!envObj["PATH"]) envObj["PATH"] = Deno.env.get("PATH") || "";

const wasiEnv = Object.entries(envObj).map(([k, v]) => `${k}=${v}`);

let memory: WebAssembly.Memory;
function getMemory(): Uint8Array { return new Uint8Array(memory.buffer); }
function getView(): DataView { return new DataView(memory.buffer); }

let wasiExtAllocate: (size: number) => number;

function readString(ptr: number, len: number): string {
    return new TextDecoder().decode(getMemory().subarray(ptr, ptr + len));
}

function toHostPath(wasiPath: string, parentPath?: string): string {
    let p = wasiPath;
    if (parentPath && !p.startsWith("/")) p = parentPath + "/" + p;
    p = p.replace(/\/+/g, "/");
    
    if (p.startsWith("/home")) return p.replace("/home", hostHome);
    if (p === "/cargo.wasm") return wasmFile; // IMPORTANT: cargo.wasm exists!
    if (p === "/") return hostCwd;
    if (p.startsWith("/")) {
        if (p.length > 3 && p[2] === ":" && p[1].match(/[a-zA-Z]/)) return p.substring(1);
        return hostCwd + p;
    }
    return hostCwd + "/" + p;
}

const openFds: Map<number, { type: string, path: string, file?: Deno.FsFile, dirEntries?: any[] }> = new Map([
    [0, { type: "stdin", path: "/dev/stdin" }],
    [1, { type: "stdout", path: "/dev/stdout" }],
    [2, { type: "stderr", path: "/dev/stderr" }],
    [3, { type: "preopen", path: "/" }], 
]);

function getNextFd(): number {
    let i = 4;
    while (openFds.has(i)) i++;
    return i;
}

const wasi_snapshot_preview1 = {
    args_get: (argv_ptr: number, argv_buf_ptr: number): number => {
        let curr_ptr = argv_buf_ptr;
        for (let i = 0; i < wasiArgs.length; i++) {
            getView().setUint32(argv_ptr + i * 4, curr_ptr, true);
            const arg = new TextEncoder().encode(wasiArgs[i] + "\0");
            getMemory().set(arg, curr_ptr);
            curr_ptr += arg.length;
        }
        return 0;
    },
    args_sizes_get: (argc_ptr: number, argv_buf_size_ptr: number): number => {
        getView().setUint32(argc_ptr, wasiArgs.length, true);
        const size = wasiArgs.reduce((acc, arg) => acc + new TextEncoder().encode(arg + "\0").length, 0);
        getView().setUint32(argv_buf_size_ptr, size, true);
        return 0;
    },
    environ_get: (environ_ptr: number, environ_buf_ptr: number): number => {
        let curr_ptr = environ_buf_ptr;
        for (let i = 0; i < wasiEnv.length; i++) {
            getView().setUint32(environ_ptr + i * 4, curr_ptr, true);
            const env = new TextEncoder().encode(wasiEnv[i] + "\0");
            getMemory().set(env, curr_ptr);
            curr_ptr += env.length;
        }
        return 0;
    },
    environ_sizes_get: (environ_count_ptr: number, environ_buf_size_ptr: number): number => {
        getView().setUint32(environ_count_ptr, wasiEnv.length, true);
        const size = wasiEnv.reduce((acc, env) => acc + new TextEncoder().encode(env + "\0").length, 0);
        getView().setUint32(environ_buf_size_ptr, size, true);
        return 0;
    },
    proc_exit: (code: number) => { Deno.exit(code); },
    fd_write: (fd: number, iovs_ptr: number, iovs_len: number, nwritten_ptr: number): number => {
        const entry = openFds.get(fd);
        if (!entry) return 8;
        let total = 0;
        for (let i = 0; i < iovs_len; i++) {
            const ptr = getView().getUint32(iovs_ptr + i * 8, true);
            const len = getView().getUint32(iovs_ptr + i * 8 + 4, true);
            const data = getMemory().subarray(ptr, ptr + len);
            if (fd === 1) Deno.stdout.writeSync(data);
            else if (fd === 2) Deno.stderr.writeSync(data);
            else if (entry.file) entry.file.writeSync(data);
            total += len;
        }
        getView().setUint32(nwritten_ptr, total, true);
        return 0;
    },
    fd_read: (fd: number, iovs_ptr: number, iovs_len: number, nread_ptr: number): number => {
        const entry = openFds.get(fd);
        if (!entry || !entry.file) return 8;
        let total = 0;
        for (let i = 0; i < iovs_len; i++) {
            const ptr = getView().getUint32(iovs_ptr + i * 8, true);
            const len = getView().getUint32(iovs_ptr + i * 8 + 4, true);
            const nread = entry.file.readSync(getMemory().subarray(ptr, ptr + len));
            if (nread === null) break;
            total += nread;
            if (nread < len) break;
        }
        getView().setUint32(nread_ptr, total, true);
        return 0;
    },
    fd_close: (fd: number): number => {
        const entry = openFds.get(fd);
        if (entry?.file) entry.file.close();
        openFds.delete(fd);
        return 0;
    },
    fd_seek: (fd: number, offset: bigint, whence: number, newoffset_ptr: number): number => {
        const entry = openFds.get(fd);
        if (!entry || !entry.file) return 8;
        const res = entry.file.seekSync(offset, whence === 0 ? "start" : whence === 1 ? "current" : "end");
        getView().setBigUint64(newoffset_ptr, res, true);
        return 0;
    },
    fd_fdstat_get: (fd: number, ptr: number) => {
        const entry = openFds.get(fd);
        if (!entry) return 8;
        const v = getView();
        v.setUint8(ptr, entry.type === "file" ? 4 : (entry.type === "dir" || entry.type === "preopen" ? 3 : 2));
        v.setUint16(ptr + 2, 0);
        v.setBigUint64(ptr + 8, 0xffffffffffffffffn, true);
        v.setBigUint64(ptr + 16, 0xffffffffffffffffn, true);
        return 0;
    },
    fd_prestat_get: (fd: number, ptr: number) => {
        const entry = openFds.get(fd);
        if (entry?.type === "preopen") {
            getView().setUint8(ptr, 0);
            getView().setUint32(ptr + 4, new TextEncoder().encode(entry.path).length, true);
            return 0;
        }
        return 8; 
    },
    fd_prestat_dir_name: (fd: number, ptr: number, _len: number) => {
        const entry = openFds.get(fd);
        if (entry?.path) { getMemory().set(new TextEncoder().encode(entry.path), ptr); return 0; }
        return 8;
    },
    path_open: (fd: number, _dirflags: number, path_ptr: number, path_len: number, oflags: number, _fs_rights_base: bigint, _fs_rights_inheriting: bigint, fdflags: number, opened_fd_ptr: number): number => {
        const p = readString(path_ptr, path_len);
        const parent = openFds.get(fd);
        if (!parent) return 8;
        try {
            const hostPath = toHostPath(p, parent.path);
            const options: Deno.OpenOptions = { read: true, write: true };
            if (oflags & 1) options.create = true;
            if (oflags & 4) options.createNew = true;
            if (oflags & 8) options.truncate = true;
            if (fdflags & 1) options.append = true;

            if (oflags & 2) {
                const stat = Deno.statSync(hostPath);
                if (!stat.isDirectory) return 54;
                const newFd = getNextFd();
                openFds.set(newFd, { type: "dir", path: hostPath });
                getView().setUint32(opened_fd_ptr, newFd, true);
                return 0;
            }
            const file = Deno.openSync(hostPath, options);
            const newFd = getNextFd();
            openFds.set(newFd, { type: "file", file, path: hostPath });
            getView().setUint32(opened_fd_ptr, newFd, true);
            return 0;
        } catch (_) { return 44; }
    },
    path_filestat_get: (fd: number, _flags: number, path_ptr: number, path_len: number, ptr: number): number => {
        const p = readString(path_ptr, path_len);
        const parent = openFds.get(fd);
        if (!parent) return 8;
        try {
            const hostPath = toHostPath(p, parent.path);
            const stat = Deno.statSync(hostPath);
            const v = getView();
            v.setBigUint64(ptr, 0n, true); v.setBigUint64(ptr + 8, 0n, true);
            v.setUint8(ptr + 16, stat.isDirectory ? 3 : 4);
            v.setBigUint64(ptr + 24, 1n, true); v.setBigUint64(ptr + 32, BigInt(stat.size), true);
            v.setBigUint64(ptr + 40, BigInt(stat.atime?.getTime() || 0) * 1000000n, true);
            v.setBigUint64(ptr + 48, BigInt(stat.mtime?.getTime() || 0) * 1000000n, true);
            v.setBigUint64(ptr + 56, BigInt(stat.birthtime?.getTime() || 0) * 1000000n, true);
            return 0;
        } catch (_) { return 44; }
    },
    fd_filestat_get: (fd: number, ptr: number): number => {
        const entry = openFds.get(fd);
        if (!entry || !entry.file) return 8;
        try {
            const stat = entry.file.statSync();
            const v = getView();
            v.setBigUint64(ptr, 0n, true); v.setBigUint64(ptr + 8, 0n, true);
            v.setUint8(ptr + 16, stat.isDirectory ? 3 : 4);
            v.setBigUint64(ptr + 24, 1n, true); v.setBigUint64(ptr + 32, BigInt(stat.size), true);
            v.setBigUint64(ptr + 40, BigInt(stat.atime?.getTime() || 0) * 1000000n, true);
            v.setBigUint64(ptr + 48, BigInt(stat.mtime?.getTime() || 0) * 1000000n, true);
            v.setBigUint64(ptr + 56, BigInt(stat.birthtime?.getTime() || 0) * 1000000n, true);
            return 0;
        } catch (_) { return 1; }
    },
    path_create_directory: (fd: number, path_ptr: number, path_len: number): number => {
        const p = readString(path_ptr, path_len);
        const parent = openFds.get(fd);
        if (!parent) return 8;
        try { Deno.mkdirSync(toHostPath(p, parent.path)); return 0; } catch (_) { return 1; }
    },
    fd_readdir: (fd: number, buf_ptr: number, buf_len: number, cookie: bigint, nwritten_ptr: number): number => {
        const entry = openFds.get(fd);
        if (!entry || (entry.type !== "dir" && entry.type !== "preopen")) return 8;
        try {
            if (cookie === 0n) entry.dirEntries = Array.from(Deno.readDirSync(entry.path));
            const entries = entry.dirEntries || [];
            let nwritten = 0, i = Number(cookie);
            while (i < entries.length && nwritten + 24 < buf_len) {
                const e = entries[i];
                const nameBuf = new TextEncoder().encode(e.name);
                if (nwritten + 24 + nameBuf.length > buf_len) break;
                getView().setBigUint64(buf_ptr + nwritten, BigInt(i + 1), true);
                getView().setBigUint64(buf_ptr + nwritten + 8, 0n, true);
                getView().setUint32(buf_ptr + nwritten + 16, nameBuf.length, true);
                getView().setUint8(buf_ptr + nwritten + 20, e.isDirectory ? 3 : 4);
                getMemory().set(nameBuf, buf_ptr + nwritten + 24);
                nwritten += 24 + nameBuf.length; i++;
            }
            getView().setUint32(nwritten_ptr, nwritten, true);
            return 0;
        } catch (_) { return 1; }
    },
    random_get: (ptr: number, len: number): number => { crypto.getRandomValues(getMemory().subarray(ptr, ptr + len)); return 0; },
    clock_time_get: (_id: number, _precision: bigint, ptr: number) => { getView().setBigUint64(ptr, BigInt(Date.now()) * 1000000n, true); return 0; },
    poll_oneoff: () => 0,
    fd_datasync: () => 0,
    fd_sync: () => 0,
    fd_fdstat_set_flags: () => 0,
    fd_fdstat_set_rights: () => 0,
    fd_advise: () => 0,
    fd_allocate: () => 0,
    path_link: () => 0,
    path_readlink: () => 0,
    path_remove_directory: () => 0,
    path_rename: () => 0,
    path_symlink: () => 0,
    path_unlink_file: () => 0,
    path_filestat_set_times: () => 0,
    fd_filestat_set_size: () => 0,
    fd_filestat_set_times: () => 0,
    fd_renumber: () => 0,
    fd_tell: () => 0,
    proc_raise: () => 0,
    sched_yield: () => 0,
    sock_recv: () => 0,
    sock_send: () => 0,
    sock_shutdown: () => 0,
    sock_accept: () => 0,
};

const envImports = {
    wasi_ext_fetch: (methodPtr: number, methodLen: number, urlPtr: number, urlLen: number, _hPtr: number, _hLen: number, _bPtr: number, _bLen: number, outStatus: number, outRespPtr: number, outRespLen: number): number => {
        try {
            const method = readString(methodPtr, methodLen);
            const url = readString(urlPtr, urlLen);
            console.log(`[Host] Fetching: ${method} ${url}`);
            const cmd = new Deno.Command("curl", { args: ["-s", "-i", "-X", method, url], stdout: "piped" });
            const output = cmd.outputSync();
            const ptr = wasiExtAllocate(output.stdout.length);
            getMemory().set(output.stdout, ptr);
            getView().setUint16(outStatus, 200, true);
            getView().setUint32(outRespPtr, ptr, true);
            getView().setUint32(outRespLen, output.stdout.length, true);
            return 0;
        } catch (_) { return 1; }
    },
    wasi_ext_spawn: (programPtr: number, programLen: number, argsPtr: number, argsLen: number, envPtr: number, envLen: number, _cwdPtr: number, _cwdLen: number, outExitCode: number, outStdoutPtr: number, outStdoutLen: number, outStderrPtr: number, outStderrLen: number): number => {
        try {
            const program = readString(programPtr, programLen);
            const sub = getMemory().subarray(argsPtr, argsPtr + argsLen);
            const args: string[] = []; let st = 0;
            for (let i = 0; i < sub.length; i++) { if (sub[i] === 0) { args.push(new TextDecoder().decode(sub.subarray(st, i))); st = i + 1; } }
            console.log(`[Host] Spawning: ${program} ${args.join(' ')}`);
            const cmd = new Deno.Command(program, { args, stdout: "piped", stderr: "piped" });
            const output = cmd.outputSync();
            const sPtr = wasiExtAllocate(output.stdout.length); getMemory().set(output.stdout, sPtr);
            const ePtr = wasiExtAllocate(output.stderr.length); getMemory().set(output.stderr, ePtr);
            getView().setInt32(outExitCode, output.code, true);
            getView().setUint32(outStdoutPtr, sPtr, true);
            getView().setUint32(outStdoutLen, output.stdout.length, true);
            getView().setUint32(outStderrPtr, ePtr, true);
            getView().setUint32(outStderrLen, output.stderr.length, true);
            return 0;
        } catch (e) { console.error("[Host Spawn Error]", e); return 1; }
    },
    wasi_ext_git_clone: () => 0,
    wasi_ext_git_fetch: () => 0,
    wasi_ext_allocate: (size: number) => wasiExtAllocate(size),
};

const module = await WebAssembly.compile(wasmBytes);
const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi_snapshot_preview1 as any, env: envImports });
memory = instance.exports.memory as WebAssembly.Memory;
wasiExtAllocate = instance.exports.wasi_ext_allocate as any;
(instance.exports._start as Function)();

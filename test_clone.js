const sb = new SharedArrayBuffer(1024);
console.log("Cloning...");
structuredClone(sb);
console.log("Cloned!");

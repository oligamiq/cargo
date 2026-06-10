const cmd = new Deno.Command("/home/oligami/.cargo/bin/rustc", {
    args: ["-vV"],
    env: { HOME: "/home/oligami" }
});
console.log(cmd.outputSync());

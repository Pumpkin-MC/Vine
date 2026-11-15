use wasmtime::component::bindgen;

bindgen!({
    path: "wit",
    world: "plugin",
    imports: {
        "vine:plugin/ipc@0.1.0.send-ipc-message": async | store | trappable,
        "vine:plugin/proxy@0.1.0.execute-command": async | store | trappable,
        default: async | trappable,
    },
    exports: {
        default: async | store | trappable,
    },
});

;; Generated fixture: valid manifest; scan spins forever (fuel must kill it)
;; plugin_meta returns a pointer to a length-prefixed manifest at 1024.
;; scan returns (ptr<<32|len) pointing at the raw findings JSON at 4096.
(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "\33\00\00\00" "{\"id\":\"FIXTURE\",\"tier\":3,\"category\":\"test\",\"abi\":1}")
  (data (i32.const 4096) "[{\"slug\":\"demo\",\"span\":[0,4],\"message\":\"demo hit\",\"advice\":null}]")
  (func (export "plugin_meta") (result i32) (i32.const 1024))
  (func (export "alloc") (param $n i32) (result i32) (i32.const 32768))
  (func (export "scan") (param i32 i32) (result i64)
    (loop (result i64) (br 0)))
)

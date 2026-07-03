(module (@rwat)
  (import "env" "add" (func $add (@sym) (param i32 i32) (result i32)))

  (func $main (@sym) (param i32 i32) (result i32)
    local.get 0
    local.get 1
    call $add (@reloc)
  )
)

(module (@rwat)
  (type $add_type (func (param i32 i32) (result i32)))
  (type $main_type (func (param i32 i32) (result i32)))

  (import "env" "add" (func $add (@sym (name "add")) (type $add_type)))

  (func $main (@sym (name "main")) (type $main_type)
    local.get 0
    local.get 1
    call $add (@reloc)
  )
)

(module (@rwat)
  (type $add_type (func (param i32 i32) (result i32)))

  (func $add (@sym (name "add")) (type $add_type)
    local.get 0
    local.get 1
    i32.add
  )
)

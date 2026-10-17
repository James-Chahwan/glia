package apperr

// Refusal shares services.Refusal's name and Text method: the composite
// `services.Refusal{..}` in this directory's test must not type as it.
type Refusal struct{}

func (Refusal) Text() string { return "apperr" }

// Error is built only by composite literals: the runtime calls its Error
// method (the `error` interface) once something builds one.
type Error struct{ Code string }

func (e *Error) Error() string { return e.Code }

var notAvailable = &Error{Code: "not_available"}

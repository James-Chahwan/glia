// jQuery: DOM events on elements.
export function wire(form) {
  $(form).on("change", validate);
  $(form).trigger("submit");
}

function validate() {}

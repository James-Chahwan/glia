(ns myapp.core
  "Core namespace docs.")

(defprotocol Greeter
  "Things that greet."
  (greet-all [this]))

(defn greet
  "Returns a greeting for name."
  [name]
  (str "hi " name))

(def banner "not a doc")

;; leading comment
(defn plain [x] x)

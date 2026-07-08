(ns app.core
  (:require [app.util :as util]
            [clojure.string :as str]))

(defn run [x]
  (util/process x))

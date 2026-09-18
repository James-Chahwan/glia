(ns app.handler
  (:require [compojure.core :refer [GET defroutes]]))

(defn list-bolts [] "ok")

(defroutes app-routes
  (GET "bolts" [] (list-bolts)))

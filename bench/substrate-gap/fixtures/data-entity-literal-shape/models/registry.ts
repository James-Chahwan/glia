// Mongoose models: one named by a literal (the control), one by a variable.
import mongoose from "mongoose";

const userSchema = new mongoose.Schema({ name: String });
export const User = mongoose.model("User", userSchema);

export function register(modelName: string, schema: mongoose.Schema) {
  return mongoose.model(modelName, schema);
}

export const REGISTRY_LABEL = "Registry";

// Astro 5 requires the collection to be declared. Without this the docs
// directory is not a collection at all and the build succeeds with a single
// page — which is what happened first time, and it reports no error.
import { defineCollection } from "astro:content";
import { docsLoader } from "@astrojs/starlight/loaders";
import { docsSchema } from "@astrojs/starlight/schema";

export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
};

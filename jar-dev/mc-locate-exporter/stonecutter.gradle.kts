plugins {
    id("dev.kikugie.stonecutter")
}

// The version whose form the checked-in source is kept in. Switch with
// `./gradlew "Set active project to <ver>"` or the stonecutter tasks.
stonecutter active "26.2.x"

stonecutter parameters {
    swaps["mod_version"] = "\"${property("mod.version")}\";"
    swaps["minecraft"] = "\"${node.metadata.version}\";"
    constants["release"] = property("mod.id") != "template"
    dependencies["fapi"] = node.project.property("deps.fabric_api") as String

    replacements {
        // Mojang renamed ResourceLocation -> Identifier in 1.21.11 and kept it
        // through 26.x. The source is written with the older name and rewritten
        // for newer versions.
        string(current.parsed >= "1.21.11") {
            replace("ResourceLocation", "Identifier")
        }
    }
}

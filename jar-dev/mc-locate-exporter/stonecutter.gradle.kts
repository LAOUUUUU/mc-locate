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

        // Access-widener header: 26.1+ uses the "official" (unobfuscated) name
        // namespace; older versions use "named".
        string(current.parsed >= "26.1") {
            replace("classTweaker v2 named", "classTweaker v2 official")
        }

        // The see-through outline needs RenderType.create, which is package-
        // private. Widen it only on 1.21.11, the one version whose new render
        // pipeline (and RenderSetup) this mod's wireframe targets; elsewhere the
        // line stays an inert "#..." comment so the widener never names a class
        // that version lacks. Loom applies the widener to the compile classpath
        // too, so the mixin can call create() directly without an @Invoker.
        string(current.parsed >= "1.21.11" && current.parsed < "26.1") {
            replace(
                "#XRAY_CREATE_DISABLED#",
                "accessible method net/minecraft/client/renderer/rendertype/RenderType "
                    + "create (Ljava/lang/String;Lnet/minecraft/client/renderer/rendertype/RenderSetup;)"
                    + "Lnet/minecraft/client/renderer/rendertype/RenderType;",
            )
        }
    }
}

plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
    kotlin("plugin.compose")
}

android {
    namespace = "dev.murmur"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.murmur"
        minSdk = 30
        targetSdk = 36
        // CI runs increment it — Obtainium-style updaters compare this,
        // not the name. Local builds stay at 1.
        versionCode = (System.getenv("GITHUB_RUN_NUMBER") ?: "1").toInt()
        // The APK always carries the release tag it was cut from, even
        // when nothing in mobile/ changed since the last one — Obtainium
        // users see "is my phone current?" against the repo's tags.
        // Chain: explicit env (CI uses GITHUB_REF_NAME) → nearest git tag
        // → dev fallback.
        versionName = run {
            val fromEnv = System.getenv("MURMUR_VERSION")
                ?: System.getenv("GITHUB_REF_NAME")?.takeIf { it.startsWith("v") }
            val fromGit = try {
                val p = ProcessBuilder("git", "describe", "--tags", "--abbrev=0")
                    .directory(rootDir.parentFile).start()
                if (p.waitFor() == 0) p.inputStream.bufferedReader().readText().trim() else null
            } catch (_: Exception) { null }
            (fromEnv ?: fromGit ?: "0.0.0-dev").removePrefix("v")
        }
        // Every phone murmur targets is arm64; the other ABIs only
        // bloated the APK with unused libsodium/JNA builds.
        ndk { abiFilters += listOf("arm64-v8a") }
    }

    signingConfigs {
        // Release signing comes from CI secrets; without them we fall
        // back to the debug key so `assembleRelease` still works locally.
        create("release") {
            val storeFile = System.getenv("MURMUR_KEYSTORE_FILE")
            if (storeFile != null) {
                this.storeFile = file(storeFile)
                storePassword = System.getenv("MURMUR_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("MURMUR_KEY_ALIAS")
                keyPassword = System.getenv("MURMUR_KEY_PASSWORD")
            } else {
                val d = signingConfigs.getByName("debug")
                this.storeFile = d.storeFile
                storePassword = d.storePassword
                keyAlias = d.keyAlias
                keyPassword = d.keyPassword
            }
        }
    }

    buildTypes {
        debug {
            // Side-by-side dev install — "murmur dev" lives next to the
            // release app, so fast local iterations never fight its
            // signing. Pause the release service before running it:
            // both would bind the same LAN ports.
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
            resValue("string", "app_name", "murmur dev")
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = signingConfigs.getByName("release")
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
    implementation("androidx.core:core-ktx:1.18.0")
    implementation("com.squareup.okhttp3:okhttp:5.4.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.13.0")
    // XChaCha20-Poly1305 — same primitive as murmur-proto on the Rust side.
    implementation("com.goterl:lazysodium-android:5.2.0@aar")
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    // UI: Compose + Material 3
    implementation(platform("androidx.compose:compose-bom:2026.06.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    debugImplementation("androidx.compose.ui:ui-tooling")

    // QR invite scanning — CameraX + MLKit. The zxing-embedded
    // Camera1 preview path doesn't decode on newer HALs (Camera2
    // native, e.g. Galaxy S26): the camera opens, frames flow, and
    // the decoder never sees a usable buffer.
    implementation("androidx.camera:camera-camera2:1.4.2")
    implementation("androidx.camera:camera-lifecycle:1.4.2")
    implementation("androidx.camera:camera-view:1.4.2")
    implementation("com.google.mlkit:barcode-scanning:17.3.0")
}

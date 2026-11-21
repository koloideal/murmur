plugins {
    id("com.android.application")
    kotlin("android")
    kotlin("plugin.serialization")
}

android {
    namespace = "dev.murmur"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.murmur"
        minSdk = 30
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
    // XChaCha20-Poly1305 — same primitive as murmur-proto on the Rust side.
    implementation("com.goterl:lazysodium-android:5.1.4@aar")
    implementation("net.java.dev.jna:jna:5.14.0@aar")
}

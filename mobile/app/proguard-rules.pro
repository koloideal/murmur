# libsodium is reached through JNA: native code looks methods and
# structures up by name, so nothing on that path may be renamed.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class com.goterl.lazysodium.** { *; }
-dontwarn java.awt.**
-dontwarn com.sun.jna.**

# Wire types are matched by @SerialName, but keep them readable in
# stack traces from the field.
-keep class dev.murmur.core.** { *; }

# zxing decodes via hints and internally-held decoder classes — R8
# renamed them once and the scanner previewed but never decoded.
-keep class com.google.zxing.** { *; }
-keep class com.journeyapps.** { *; }
-dontwarn com.google.zxing.**
-dontwarn com.journeyapps.**

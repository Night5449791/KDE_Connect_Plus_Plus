plugins {
    id("com.android.application")
}

android {
    namespace = "org.kdeconnect.rust"
    compileSdk = 35

    defaultConfig {
        applicationId = "org.kdeconnect.rust"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1"
    }

    buildTypes {
        getByName("release") {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

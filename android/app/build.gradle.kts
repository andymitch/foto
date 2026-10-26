plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.andymitch.foto"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.andymitch.foto"
        minSdk = 30
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    sourceSets["main"].java.srcDir(layout.buildDirectory.dir("generated/uniffi"))
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

val rust by tasks.registering(Exec::class) {
    val release = gradle.startParameter.taskNames.any { it.contains("Release", ignoreCase = true) }
    workingDir = rootDir.parentFile
    commandLine("./scripts/build-android.sh", if (release) "release" else "debug")
}

tasks.named("preBuild") { dependsOn(rust) }

dependencies {
    val bom = platform("androidx.compose:compose-bom:2025.06.01")
    implementation(bom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.1")
    implementation("androidx.work:work-runtime-ktx:2.10.2")
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
}

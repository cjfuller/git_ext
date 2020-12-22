plugins {
    kotlin("jvm") version "1.4.21"
    application
    id("org.mikeneck.graalvm-native-image") version "v1.0.0"
}

repositories {
    jcenter()
}

dependencies {
    implementation("com.github.ajalt.clikt:clikt:3.1.0")
    implementation("com.github.ajalt.mordant:mordant:2.0.0-alpha1")
}

application {
    mainClass.set("io.cjf.git_ext.MainKt")
}

kotlin {
    sourceSets["main"].apply {
        kotlin.srcDir("src")
    }
}

val compileKotlin: org.jetbrains.kotlin.gradle.tasks.KotlinCompile by tasks
compileKotlin.kotlinOptions.jvmTarget = "11"

nativeImage {
    graalVmHome = System.getProperty("java.home")
    mainClass = "io.cjf.git_ext.MainKt"
    executableName = "git_ext"
    outputDirectory = File("$buildDir/native")
    arguments(
        "--no-fallback"
    )
}

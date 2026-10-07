// Bed's thin accessor for the static Dear ImGui FreeType callback table.
#include "imgui.h"
#include "misc/freetype/imgui_freetype.h"
extern "C" const ImFontLoader* bed_imgui_freetype_loader()
{
    return ImGuiFreeType::GetFontLoader();
}
